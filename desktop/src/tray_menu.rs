//! macOS 27 opens a status item's attached menu on any click before tray-icon's
//! click view sees it. Releases before 0.25.1 keep the menu attached (see
//! build.rs), so there it is attached only while a right click presents it
//! (tray-icon#365).
use objc2::{rc::Retained, MainThreadMarker};
use objc2_app_kit::NSMenu;
use objc2_foundation::NSProcessInfo;
use std::cell::RefCell;
use tauri::tray::TrayIcon;

thread_local! {
    static MENU: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
}

/// Takes the menu off the status item so that left clicks reach the panel.
pub fn detach(tray: &TrayIcon) {
    if NSProcessInfo::processInfo()
        .operatingSystemVersion()
        .majorVersion
        < 27
    {
        return;
    }
    let _ = tray.with_inner_tray_icon(|inner| {
        let (Some(mtm), Some(item)) = (MainThreadMarker::new(), inner.ns_status_item()) else {
            return;
        };
        MENU.with(|menu| *menu.borrow_mut() = item.menu(mtm));
        item.setMenu(None);
    });
}

/// Presents a detached menu under the status item, as a click on an attached
/// menu does.
pub fn show(tray: &TrayIcon) {
    let _ = tray.with_inner_tray_icon(|inner| {
        let (Some(mtm), Some(item)) = (MainThreadMarker::new(), inner.ns_status_item()) else {
            return;
        };
        let (Some(menu), Some(button)) =
            (MENU.with(|menu| menu.borrow().clone()), item.button(mtm))
        else {
            return;
        };
        item.setMenu(Some(&menu));
        // SAFETY: `performClick:` accepts a nil sender.
        unsafe { button.performClick(None) };
        item.setMenu(None);
    });
}
