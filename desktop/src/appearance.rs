//! System-owned backdrop plus semantic WebKit colors, as used by Coport.
//! Keep the material behind the WebView; an opaque HTML panel would hide it.
use tauri::{App, Manager};

pub fn configure(app: &mut App) -> tauri::Result<()> {
    // Non-macOS release builds do not need any window appearance overrides.
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
