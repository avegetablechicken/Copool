//! System-owned backdrop plus semantic WebKit colors, as used by Coport.
//! Keep the material behind the WebView; an opaque HTML panel would hide it.
use tauri::{App, Manager};

pub fn configure(app: &mut App) -> tauri::Result<()> {
    // Other platforms only report to the UI (Linux) whether WebKit composites.
    if let Some(_window) = app.get_webview_window("main") {
        #[cfg(target_os = "macos")]
        {
            use tauri::window::{Effect, EffectState, EffectsBuilder};
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
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
        // An isolated smoke-test bundle can exercise both native appearances.
        // Release builds always follow the OS, like the SwiftUI application.
        #[cfg(debug_assertions)]
        match std::env::var("COPOOL_PREVIEW_APPEARANCE").as_deref() {
            Ok("dark") => _window.set_theme(Some(tauri::Theme::Dark))?,
            Ok("light") => _window.set_theme(Some(tauri::Theme::Light))?,
            _ => {}
        }
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
