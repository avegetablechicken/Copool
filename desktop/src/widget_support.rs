use copool_core::{
    widgets::{Snapshot, Widget},
    AccountSummary, Application, Pool,
};
use std::sync::Arc;

#[cfg(target_os = "macos")]
extern "C" {
    fn copool_publish_widget(
        filename: *const std::ffi::c_char,
        kind: *const std::ffi::c_char,
        bytes: *const u8,
        length: usize,
    ) -> i32;
}

/// Always write an inspectable portable snapshot of each widget. Native
/// publishing is disabled for isolated test data, so a smoke test cannot
/// replace the user's widgets.
pub async fn publish(app: &Arc<Application>) -> Result<(), String> {
    let listed = |accounts: Vec<AccountSummary>| {
        accounts
            .into_iter()
            .filter(|a| a.display_status == "list")
            .collect::<Vec<_>>()
    };
    let mut pools = Vec::new();
    for pool in Pool::ALL {
        pools.push((pool, app.accounts(pool).await.map(listed)));
    }
    let preferences = app.preferences()?;
    let now = chrono::Local::now().timestamp();
    let mut errors = vec![];
    // A pool that cannot be read leaves the widgets showing it as they were.
    for widget in Widget::ALL {
        let shown: Option<Vec<(Pool, &[AccountSummary])>> = widget
            .pools()
            .iter()
            .map(|pool| match pools.iter().find(|(p, _)| p == pool) {
                Some((_, Ok(accounts))) => Some((*pool, accounts.as_slice())),
                _ => None,
            })
            .collect();
        let Some(shown) = shown else {
            continue;
        };
        let snapshot = widget.build(&shown, &preferences.usage_display, None, now);
        if let Err(error) = publish_widget(app, widget, &snapshot) {
            errors.push(error);
        }
    }
    errors.extend(pools.into_iter().filter_map(|(_, result)| result.err()));
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn publish_widget(app: &Application, widget: Widget, snapshot: &Snapshot) -> Result<(), String> {
    let path = app.paths.data_dir.join(widget.filename());
    let native_result = std::cell::RefCell::new(Ok(()));
    copool_core::widgets::write(widget, &path, snapshot, |kind| {
        #[cfg(target_os = "macos")]
        if std::env::var_os("COPOOL_DATA_DIR").is_none() {
            *native_result.borrow_mut() = (|| {
                let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                let filename = std::ffi::CString::new(widget.filename()).unwrap();
                let kind = std::ffi::CString::new(kind).unwrap();
                // The bridge reads these synchronously and never retains them.
                let result = unsafe {
                    copool_publish_widget(
                        filename.as_ptr(),
                        kind.as_ptr(),
                        bytes.as_ptr(),
                        bytes.len(),
                    )
                };
                if result == 0 {
                    Ok(())
                } else {
                    Err(format!("WidgetKit snapshot publishing failed ({result})"))
                }
            })();
        }
        #[cfg(not(target_os = "macos"))]
        let _ = kind;
    })?;
    native_result.into_inner()
}
