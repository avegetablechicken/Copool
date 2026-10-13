use copool_core::Application;
use std::sync::Arc;

#[cfg(target_os = "macos")]
extern "C" {
    fn copool_publish_widget(bytes: *const u8, length: usize) -> i32;
}

/// Always write an inspectable portable snapshot. Native publishing is disabled
/// for isolated test data, so a smoke test cannot replace the user's widget.
pub async fn publish(app: &Arc<Application>) -> Result<(), String> {
    let accounts = app
        .accounts()
        .await?
        .into_iter()
        .filter(|a| a.display_status == "list")
        .collect::<Vec<_>>();
    let preferences = app.preferences()?;
    let now = chrono::Local::now();
    let path = app.paths.data_dir.join("accounts-widget-snapshot.json");
    let native_result = std::cell::RefCell::new(Ok(()));
    copool_core::widgets::write(
        &path,
        &accounts,
        &preferences.usage_display,
        None,
        now.timestamp(),
        |_| {
            #[cfg(target_os = "macos")]
            if std::env::var_os("COPOOL_DATA_DIR").is_none() {
                *native_result.borrow_mut() = (|| {
                    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                    // The bridge reads this slice synchronously and never retains it.
                    let result = unsafe { copool_publish_widget(bytes.as_ptr(), bytes.len()) };
                    if result == 0 {
                        Ok(())
                    } else {
                        Err(format!("WidgetKit snapshot publishing failed ({result})"))
                    }
                })();
            }
        },
    )?;
    native_result.into_inner()
}
