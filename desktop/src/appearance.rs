//! System-owned backdrop plus semantic WebKit colors, as used by Coport.
//! Keep the material behind the WebView; an opaque HTML panel would hide it.
use tauri::{App, AppHandle, Manager, Theme};

pub fn window_theme(value: &str) -> Option<Theme> {
    match value {
        "dark" => Some(Theme::Dark),
        "light" => Some(Theme::Light),
        _ => None,
    }
}

pub fn apply_theme(app: &AppHandle, value: &str) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.set_theme(window_theme(value))?;
    }
    Ok(())
}

pub fn apply_initial_theme(app: &AppHandle, saved: &str) -> tauri::Result<()> {
    // An isolated smoke-test bundle can override the saved appearance.
    #[cfg(debug_assertions)]
    if let Ok(preview) = std::env::var("COPOOL_PREVIEW_APPEARANCE") {
        if matches!(preview.as_str(), "dark" | "light") {
            return apply_theme(app, &preview);
        }
    }
    apply_theme(app, saved)
}

pub fn configure(app: &mut App) -> tauri::Result<()> {
    // Other platforms only report to the UI (Linux) whether WebKit composites.
    if let Some(_window) = app.get_webview_window("main") {
        #[cfg(target_os = "macos")]
        {
            use tauri::window::{Effect, EffectState, EffectsBuilder};
            _window.set_effects(
                EffectsBuilder::new()
                    .effect(Effect::Popover)
                    .state(EffectState::Active)
                    .radius(12.0)
                    .build(),
            )?;
        }
        #[cfg(target_os = "linux")]
        _window.with_webview(|webview| {
            use webkit2gtk::{HardwareAccelerationPolicy, SettingsExt, WebViewExt};
            // WebKit reports `Never` when it cannot composite on this system.
            let policy = WebViewExt::settings(&webview.inner())
                .map(|settings| settings.hardware_acceleration_policy());
            crate::backdrop::set_composited(
                policy.is_some_and(|policy| policy != HardwareAccelerationPolicy::Never),
            );
        })?;
    }
    Ok(())
}

/// WebKitGTK cannot share GPU buffers with NVIDIA's proprietary driver, so it
/// draws without compositing, which ignores `backdrop-filter`. Compositing on
/// the GPU and handing frames over in shared memory keeps it. Settings the
/// user made for these renderers win.
#[cfg(target_os = "linux")]
pub fn enable_compositing() {
    const SETTINGS: [&str; 4] = [
        "WEBKIT_DISABLE_COMPOSITING_MODE",
        "WEBKIT_DISABLE_DMABUF_RENDERER",
        "WEBKIT_FORCE_DMABUF_RENDERER",
        "WEBKIT_DMABUF_RENDERER_FORCE_SHM",
    ];
    if !std::path::Path::new("/sys/module/nvidia").exists()
        || SETTINGS.iter().any(|name| std::env::var_os(name).is_some())
    {
        return;
    }
    // Called first in main, before any thread or WebKit process starts.
    std::env::set_var("WEBKIT_FORCE_DMABUF_RENDERER", "1");
    std::env::set_var("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_theme_preferences_to_native_window_themes() {
        assert_eq!(window_theme("dark"), Some(Theme::Dark));
        assert_eq!(window_theme("light"), Some(Theme::Light));
        assert_eq!(window_theme("system"), None);
        assert_eq!(window_theme("unexpected"), None);
    }
}
