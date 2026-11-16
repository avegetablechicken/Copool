//! Launch at startup through `SMAppService.mainApp` (macOS 13 and later), as
//! the original app did. A launch agent plist outside the bundle is listed
//! under the developer's name in System Settings, where it is easily blocked.

use objc2::{
    msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject},
};
use objc2_foundation::NSError;

// SMAppServiceStatus values.
const ENABLED: isize = 1;
const REQUIRES_APPROVAL: isize = 2;

#[link(name = "ServiceManagement", kind = "framework")]
extern "C" {}

/// The app's login item; missing before macOS 13 or outside an app bundle.
fn main_app() -> Option<Retained<AnyObject>> {
    let bundled = std::env::current_exe()
        .ok()?
        .ancestors()
        .nth(3)
        .is_some_and(|bundle| bundle.extension().is_some_and(|ext| ext == "app"));
    if !bundled {
        return None;
    }
    let class = AnyClass::get(c"SMAppService")?;
    // SAFETY: a class property of SMAppService.
    unsafe { msg_send![class, mainAppService] }
}

fn status(service: &AnyObject) -> isize {
    // SAFETY: `status` is a read-only NSInteger property of SMAppService.
    unsafe { msg_send![service, status] }
}

fn error(error: Retained<NSError>) -> String {
    error.localizedDescription().to_string()
}

/// Whether the login item applies here; otherwise the core's startup entry is used.
pub fn available() -> bool {
    main_app().is_some()
}

/// Registered, including when System Settings blocks it.
pub fn is_enabled() -> bool {
    main_app().is_some_and(|service| matches!(status(&service), ENABLED | REQUIRES_APPROVAL))
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let service = main_app().ok_or("Launch at startup is unavailable.")?;
    if enabled {
        if status(&service) == ENABLED {
            return Ok(());
        }
        // SAFETY: an SMAppService method returning BOOL with an NSError out-parameter.
        let result: Result<(), Retained<NSError>> =
            unsafe { msg_send![&*service, registerAndReturnError: _] };
        // A login item switched off in System Settings stays registered,
        // awaiting approval there.
        match result {
            Err(e) if status(&service) != REQUIRES_APPROVAL => Err(error(e)),
            _ => Ok(()),
        }
    } else if matches!(status(&service), ENABLED | REQUIRES_APPROVAL) {
        // SAFETY: as for registering.
        let result: Result<(), Retained<NSError>> =
            unsafe { msg_send![&*service, unregisterAndReturnError: _] };
        result.map_err(error)
    } else {
        Ok(())
    }
}
