#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use copool_core::{
    AccountSummary, ApiProxyStatus, Application, Paths, Pool, Preferences, SmartSwitchOutcome,
    SwitchExecution,
};
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, State,
};
mod appearance;
#[cfg(target_os = "linux")]
mod backdrop;
#[cfg(target_os = "macos")]
mod login_item;
mod native_icons;
mod panel;
mod placement;
#[cfg(target_os = "macos")]
mod status_item;
mod widget_support;
type App = Arc<Application>;

/// Uploads a pool's accounts to its remote servers after they changed.
fn sync_remote_after_mutation(handle: tauri::AppHandle, app: App, pool: Pool) {
    tauri::async_runtime::spawn(async move {
        let Ok(settings) = app.configuration() else {
            return;
        };
        let mut failures = vec![];
        for server in settings.remote_servers {
            let server_pool =
                serde_json::from_value::<Pool>(server["pool"].clone()).unwrap_or_default();
            if server_pool != pool {
                continue;
            }
            if let Some(id) = server["id"].as_str() {
                if let Err(error) = app.remote_action(id, "sync", None).await {
                    failures.push(format!(
                        "{}: {error}",
                        server["label"].as_str().unwrap_or(id)
                    ));
                }
            }
        }
        if !failures.is_empty() {
            let _ = handle.emit("remote-sync-error", failures.join("\n"));
        }
    });
}
#[tauri::command]
async fn sync_remote_accounts(
    handle: tauri::AppHandle,
    app: State<'_, App>,
    pool: Pool,
) -> Result<(), String> {
    sync_remote_after_mutation(handle, app.inner().clone(), pool);
    Ok(())
}
#[tauri::command]
async fn accounts(app: State<'_, App>, pool: Pool) -> Result<Vec<AccountSummary>, String> {
    app.accounts(pool).await
}
#[tauri::command]
async fn import_json(app: State<'_, App>, pool: Pool, content: String) -> Result<usize, String> {
    app.import_json(pool, &content).await
}
#[tauri::command]
async fn import_current(app: State<'_, App>, pool: Pool) -> Result<usize, String> {
    app.import_current(pool).await
}
/// Imports a login file the user picks; none when the picker is cancelled,
/// which reports nothing, as in Swift.
#[tauri::command]
async fn import_account_file(
    handle: tauri::AppHandle,
    app: State<'_, App>,
    pool: Pool,
) -> Result<Option<usize>, String> {
    let Some(path) = pick_file(&handle, Some(("JSON", &["json"])), None).await? else {
        return Ok(None);
    };
    app.import_file(pool, &path).await.map(Some)
}
#[tauri::command]
async fn delete_account(app: State<'_, App>, pool: Pool, id: String) -> Result<(), String> {
    app.delete_account(pool, &id).await
}
#[tauri::command]
async fn switch_account(
    app: State<'_, App>,
    pool: Pool,
    id: String,
) -> Result<SwitchExecution, String> {
    app.switch_account(pool, &id).await
}
#[tauri::command]
async fn smart_switch(app: State<'_, App>, pool: Pool) -> Result<SmartSwitchOutcome, String> {
    app.smart_switch(pool).await
}
#[tauri::command]
async fn refresh_usage(app: State<'_, App>, pool: Pool, id: Option<String>) -> Result<(), String> {
    app.refresh_now(pool, id.as_deref()).await
}
/// The next background refresh of the whole pool.
#[tauri::command]
fn next_full_refresh_at(app: State<'_, App>, pool: Pool) -> Option<i64> {
    app.next_full_refresh_at(pool)
}
#[tauri::command]
async fn say_hi(app: State<'_, App>, pool: Pool, id: String) -> Result<(), String> {
    let _ = app.authorize(pool).await;
    app.release_new_accounts(pool, Some(&id)).await?;
    app.say_hi(pool, &id).await
}
#[tauri::command]
async fn renew_now(app: State<'_, App>, pool: Pool, id: String) -> Result<(), String> {
    let _ = app.authorize(pool).await;
    app.renew_now(pool, &id).await
}
#[tauri::command]
async fn save_account_proxy(
    app: State<'_, App>,
    pool: Pool,
    id: String,
    value: String,
) -> Result<(), String> {
    app.save_account_proxy(pool, &id, &value).await
}
#[tauri::command]
async fn set_renewal(
    app: State<'_, App>,
    pool: Pool,
    id: String,
    renewal: String,
) -> Result<(), String> {
    app.set_renewal(pool, &id, &renewal).await
}
#[tauri::command]
async fn greeting_schedule(
    app: State<'_, App>,
    pool: Pool,
    id: String,
) -> Result<copool_core::GreetingSchedule, String> {
    app.greeting_schedule(pool, &id).await
}
#[tauri::command]
async fn set_greeting_schedule(
    app: State<'_, App>,
    pool: Pool,
    id: String,
    auto: bool,
    times: Vec<String>,
) -> Result<(), String> {
    app.set_greeting_schedule(pool, &id, auto, times).await
}
#[tauri::command]
async fn proxy_status(app: State<'_, App>, pool: Pool) -> Result<ApiProxyStatus, String> {
    app.proxy_status(pool).await
}
#[tauri::command]
async fn start_proxy(
    app: State<'_, App>,
    pool: Pool,
    port: Option<u16>,
) -> Result<ApiProxyStatus, String> {
    if let Some(port) = port {
        let mut preferences = app.preferences()?;
        match pool {
            Pool::Codex => preferences.proxy_port = port,
            Pool::Claude => preferences.claude_proxy_port = port,
            Pool::Antigravity => preferences.antigravity_proxy_port = port,
        }
        app.save_preferences(preferences).await?;
    }
    app.start_proxy(pool).await
}
/// Stops a pool's proxy, and its public tunnel with it.
#[tauri::command]
async fn stop_proxy(
    app: State<'_, App>,
    tunnels: State<'_, TunnelState>,
    pool: Pool,
) -> Result<ApiProxyStatus, String> {
    tunnels.get(pool).lock().await.stop().await?;
    app.stop_proxy(pool).await
}
#[tauri::command]
async fn rotate_proxy_key(app: State<'_, App>, pool: Pool) -> Result<ApiProxyStatus, String> {
    app.rotate_proxy_key(pool).await
}
#[tauri::command]
fn preferences(app: State<'_, App>) -> Result<Preferences, String> {
    app.preferences()
}
#[tauri::command]
async fn save_preferences(
    handle: tauri::AppHandle,
    app: State<'_, App>,
    value: Preferences,
) -> Result<(), String> {
    let old = app.preferences()?;
    if old.launch_at_startup != value.launch_at_startup {
        set_launch_at_startup(value.launch_at_startup)?;
    }
    let theme = value.theme.clone();
    app.save_preferences(value).await?;
    if old.theme != theme {
        appearance::apply_theme(&handle, &theme).map_err(|e| e.to_string())?;
    }
    Ok(())
}
/// On macOS 13 and later the app registers as a login item; elsewhere, or
/// outside an app bundle, the core writes the platform's startup entry.
fn set_launch_at_startup(enabled: bool) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    if login_item::available() {
        login_item::set_enabled(enabled)?;
        // Replaces a launch agent written by an earlier version either way.
        return copool_core::platform::startup(false, &executable);
    }
    copool_core::platform::startup(enabled, &executable)
}
/// Matches the login item to the saved preference at launch, as the original
/// app did. One blocked in System Settings still counts as registered.
#[cfg(target_os = "macos")]
fn sync_launch_at_startup(app: &Application) {
    if !login_item::available() {
        return;
    }
    let result = app.preferences().and_then(|preferences| {
        if login_item::is_enabled() == preferences.launch_at_startup {
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            copool_core::platform::startup(false, &executable)
        } else {
            set_launch_at_startup(preferences.launch_at_startup)
        }
    });
    if let Err(error) = result {
        eprintln!("{error}");
    }
}
#[tauri::command]
fn paths(app: State<'_, App>) -> Paths {
    app.paths.clone()
}
#[tauri::command]
fn configuration(app: State<'_, App>) -> Result<copool_core::settings::Settings, String> {
    app.configuration()
}
#[tauri::command]
fn current_provider(app: State<'_, App>) -> copool_core::codex_config::Definition {
    copool_core::codex_config::resolve_path(&app.codex_config_path())
}
#[tauri::command]
fn provider_definitions(app: State<'_, App>) -> Vec<copool_core::codex_config::Definition> {
    app.provider_definitions()
}
/// A pool's Sub2api providers, without their secrets.
#[tauri::command]
async fn providers(app: State<'_, App>, pool: Pool) -> Result<serde_json::Value, String> {
    let app = app.inner().clone();
    // Resolving Claude settings may scan directories; never on the UI thread.
    tokio::task::spawn_blocking(move || app.providers(pool))
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn save_provider(
    app: State<'_, App>,
    pool: Pool,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    app.save_provider(pool, value).await
}
#[tauri::command]
async fn remove_provider(app: State<'_, App>, pool: Pool, id: String) -> Result<(), String> {
    app.remove_provider(pool, &id).await
}
/// Refreshes a provider's cards; a sync adopts its accounts as they are.
#[tauri::command]
async fn refresh_provider(
    app: State<'_, App>,
    pool: Pool,
    id: String,
    sync: bool,
) -> Result<usize, String> {
    // Copool's Keychain item holds the providers' passwords.
    let _ = app.authorize_keychain().await;
    app.refresh_provider(pool, &id, sync).await
}
#[tauri::command]
fn installed_editors() -> Vec<copool_core::platform::Editor> {
    copool_core::platform::editors()
}
#[derive(Default)]
struct LoginState(tokio::sync::Mutex<Option<Arc<copool_core::login::Login>>>);
/// A browser sign-in to the pool; finish_login and cancel_login complete it.
#[tauri::command]
async fn begin_login(
    state: State<'_, LoginState>,
    pool: Pool,
    workspace: Option<String>,
) -> Result<String, String> {
    let mut current = state.0.lock().await;
    if current.is_some() {
        return Err("Login already in progress".into());
    }
    let login = copool_core::login::Login::begin(pool, workspace).await?;
    copool_core::platform::open_url(&login.url)?;
    let id = login.id.clone();
    *current = Some(login);
    Ok(id)
}
#[tauri::command]
async fn finish_login(
    handle: tauri::AppHandle,
    app: State<'_, App>,
    state: State<'_, LoginState>,
    id: String,
) -> Result<usize, String> {
    let login = state
        .0
        .lock()
        .await
        .as_ref()
        .filter(|l| l.id == id)
        .cloned()
        .ok_or("Login not found")?;
    let result = tokio::select! {
        result = async {
            let document = login.finish().await?;
            app.import_sign_in(login.pool(), document, login.workspace()).await
        } => result,
        _ = login.cancelled() => Err("error.oauth.request_cancelled".into())
    };
    let mut current = state.0.lock().await;
    if current.as_ref().is_some_and(|l| l.id == id) {
        *current = None;
    }
    if result.is_ok() {
        panel::show(&handle, None);
    }
    result
}
#[tauri::command]
async fn cancel_login(state: State<'_, LoginState>) -> Result<(), String> {
    if let Some(login) = state.0.lock().await.take() {
        login.cancel();
    }
    Ok(())
}
/// A file the user picks in the native dialog, which keeps the panel open.
async fn pick_file(
    handle: &tauri::AppHandle,
    filter: Option<(&str, &[&str])>,
    directory: Option<std::path::PathBuf>,
) -> Result<Option<std::path::PathBuf>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let mut dialog = handle.dialog().file();
    if let Some((name, extensions)) = filter {
        dialog = dialog.add_filter(name, extensions);
    }
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    panel::set_modal(handle, true);
    dialog.pick_file(move |file| {
        let _ = sender.send(file);
    });
    let picked = receiver.await;
    panel::set_modal(handle, false);
    picked
        .map_err(|e| e.to_string())?
        .map(|file| file.into_path().map_err(|e| e.to_string()))
        .transpose()
}
fn path_text(path: Option<std::path::PathBuf>) -> Option<String> {
    path.map(|p| p.to_string_lossy().into_owned())
}
#[tauri::command]
async fn pick_identity_file(handle: tauri::AppHandle) -> Result<Option<String>, String> {
    pick_file(&handle, None, None).await.map(path_text)
}
/// A provider settings file, chosen from the Claude configuration directory.
#[tauri::command]
async fn pick_settings_file(
    handle: tauri::AppHandle,
    app: State<'_, App>,
) -> Result<Option<String>, String> {
    let directory = app.claude_location().config_dir;
    pick_file(&handle, Some(("JSON", &["json"])), Some(directory))
        .await
        .map(path_text)
}
#[tauri::command]
async fn save_remote(
    app: State<'_, App>,
    value: serde_json::Value,
    previous_id: Option<String>,
) -> Result<serde_json::Value, String> {
    app.save_remote(value, previous_id.as_deref()).await
}

#[tauri::command]
async fn remove_remote(app: State<'_, App>, id: String) -> Result<(), String> {
    app.remove_remote(&id).await
}
#[tauri::command]
async fn remote_action(
    handle: tauri::AppHandle,
    app: State<'_, App>,
    id: String,
    action: String,
) -> Result<serde_json::Value, String> {
    // Status loads with the tab; the other actions are the user's.
    if action != "status" {
        let _ = app.authorize_keychain().await;
    }
    app.remote_action(&id, &action, handle.path().resource_dir().ok())
        .await
}
/// Each pool's public tunnel to its proxy.
struct TunnelState([tokio::sync::Mutex<copool_core::tunnel::Tunnel>; Pool::ALL.len()]);
impl Default for TunnelState {
    fn default() -> Self {
        Self(Pool::ALL.map(|pool| tokio::sync::Mutex::new(copool_core::tunnel::Tunnel::new(pool))))
    }
}
impl TunnelState {
    fn get(&self, pool: Pool) -> &tokio::sync::Mutex<copool_core::tunnel::Tunnel> {
        &self.0[pool as usize]
    }
    /// Stops every tunnel, for quitting.
    async fn stop_all(&self) {
        for tunnel in &self.0 {
            let _ = tunnel.lock().await.stop().await;
        }
    }
}
#[tauri::command]
async fn save_tunnel_configuration(
    app: State<'_, App>,
    pool: Pool,
    value: copool_core::tunnel::Configuration,
) -> Result<(), String> {
    app.save_tunnel_configuration(pool, &value).await
}
#[tauri::command]
async fn tunnel_action(
    app: State<'_, App>,
    tunnels: State<'_, TunnelState>,
    pool: Pool,
    action: String,
    value: Option<copool_core::tunnel::Configuration>,
) -> Result<copool_core::tunnel::Status, String> {
    let mut tunnel = tunnels.get(pool).lock().await;
    match action.as_str() {
        "install" => copool_core::tunnel::install(&app.paths.data_dir).await?,
        "start" => {
            let proxy = app.proxy_status(pool).await?;
            if !proxy.running {
                return Err("error.cloudflared.start_api_proxy_first".into());
            }
            let config = value.ok_or("Tunnel configuration missing")?;
            if !config.enabled {
                return Err("Enable public access first".into());
            }
            app.save_tunnel_configuration(pool, &config).await?;
            let _ = app.authorize_keychain().await;
            let config = app.with_tunnel_token(pool, config)?;
            return tunnel
                .start(
                    &app.paths.data_dir,
                    proxy.port.ok_or("Proxy port missing")?,
                    &config,
                )
                .await;
        }
        "stop" => tunnel.stop().await?,
        "status" => {}
        _ => return Err("Unknown tunnel operation".into()),
    }
    Ok(tunnel.status(&app.paths.data_dir))
}
#[derive(Default)]
struct WorkspaceRefreshState(tokio::sync::Mutex<Option<(String, tokio::task::AbortHandle)>>);
#[tauri::command]
async fn workspace_directory(
    app: State<'_, App>,
    state: State<'_, WorkspaceRefreshState>,
    refresh: bool,
) -> Result<Vec<serde_json::Value>, String> {
    if !refresh {
        return app.workspace_directory().await;
    }
    let mut running = state.0.lock().await;
    if let Some((_, handle)) = running.take() {
        handle.abort();
    }
    let app = app.inner().clone();
    let task = tokio::spawn(async move { app.sync_workspace_directory().await });
    let id = uuid::Uuid::new_v4().to_string();
    *running = Some((id.clone(), task.abort_handle()));
    drop(running);
    let result = task.await.map_err(|e| {
        if e.is_cancelled() {
            "cancelled".into()
        } else {
            e.to_string()
        }
    });
    let mut running = state.0.lock().await;
    if running.as_ref().is_some_and(|(current, _)| *current == id) {
        *running = None;
    }
    result?
}
#[tauri::command]
async fn cancel_workspace_refresh(state: State<'_, WorkspaceRefreshState>) -> Result<(), String> {
    if let Some((_, handle)) = state.0.lock().await.take() {
        handle.abort();
    }
    Ok(())
}
#[tauri::command]
fn refresh_activity(app: State<'_, App>) -> Vec<String> {
    app.refresh_activity()
}
#[tauri::command]
async fn refresh_workspace_names(app: State<'_, App>) -> Result<Vec<AccountSummary>, String> {
    app.accounts_with_metadata().await
}
#[tauri::command]
async fn dismiss_workspace(app: State<'_, App>, id: String) -> Result<(), String> {
    app.dismiss_workspace(&id).await
}
#[tauri::command]
async fn quit_app(app: tauri::AppHandle, tunnels: State<'_, TunnelState>) -> Result<(), String> {
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), tunnels.stop_all()).await;
    app.exit(0);
    Ok(())
}
#[tauri::command]
fn fit_panel(app: tauri::AppHandle, height: f64) {
    panel::fit(&app, height);
}
#[tauri::command]
fn hide_panel(app: tauri::AppHandle) {
    panel::hide(&app);
}
/// Moves menu bar keyboard navigation from the open panel to the neighbouring
/// item (macOS); elsewhere the panel is not part of such navigation.
#[tauri::command]
fn navigate_menu_bar(app: tauri::AppHandle, right: bool) {
    #[cfg(target_os = "macos")]
    status_item::navigate(&app, right);
    #[cfg(not(target_os = "macos"))]
    let _ = (app, right);
}
fn main() {
    #[cfg(target_os = "linux")]
    appearance::enable_compositing();
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut app = tauri::Builder::default()
        .on_window_event(panel::on_window_event)
        .manage(panel::PanelState::default())
        .manage(LoginState::default())
        .manage(TunnelState::default())
        .manage(WorkspaceRefreshState::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            // Called on a D-Bus thread; showing the panel uses GTK.
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || panel::show(&handle, None));
        }))
        .setup(|app| {
            appearance::configure(app)?;
            app.manage(native_icons::load().map_err(std::io::Error::other)?);
            let service = Application::open(Paths::discover()?)
                .map_err(std::io::Error::other)?
                .into_shared();
            if let Ok(preferences) = service.preferences() {
                appearance::apply_initial_theme(app.handle(), &preferences.theme)?;
            }
            let handle = app.handle().clone();
            let (widget_tx, mut widget_rx) = tokio::sync::mpsc::channel::<()>(1);
            let widget_app = service.clone();
            tauri::async_runtime::spawn(async move {
                while widget_rx.recv().await.is_some() {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    while widget_rx.try_recv().is_ok() {}
                    if let Err(error) = widget_support::publish(&widget_app).await {
                        eprintln!("{error}");
                    }
                }
            });
            let _ = widget_tx.try_send(());
            service.on_change(Arc::new(move || {
                let _ = handle.emit("copool-changed", ());
                let _ = widget_tx.try_send(());
            }));
            service.enable_desktop_effects();
            #[cfg(target_os = "macos")]
            sync_launch_at_startup(&service);
            app.manage(service.clone());
            tauri::async_runtime::spawn(service.background());
            let open = MenuItem::with_id(app, "open", "Open Copool", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Copool", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            #[cfg(target_os = "macos")]
            let tray_image = native_icons::tray().map_err(std::io::Error::other)?;
            #[cfg(not(target_os = "macos"))]
            let tray_image = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))?;
            // Left click toggles the panel under the icon, like a menu.
            let tray = TrayIconBuilder::new()
                .icon(tray_image)
                .icon_as_template(cfg!(target_os = "macos"))
                .tooltip("Copool")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| match event {
                    tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        rect,
                        position,
                        ..
                    } => panel::toggle(tray.app_handle(), Some(panel::anchor(rect, position))),
                    #[cfg(all(target_os = "macos", tray_icon_attached_menu))]
                    tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Right,
                        button_state: tauri::tray::MouseButtonState::Down,
                        ..
                    } => status_item::show_menu(),
                    _ => {}
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        // GNOME fades the tray menu out (150 ms) after activation;
                        // waiting keeps it out of the panel's captured backdrop.
                        // The pointer is still on the menu, under the tray icon.
                        #[cfg(target_os = "linux")]
                        {
                            let anchor = panel::menu_anchor(app);
                            let handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                                let app = handle.clone();
                                let _ =
                                    handle.run_on_main_thread(move || panel::show(&app, anchor));
                            });
                        }
                        #[cfg(not(target_os = "linux"))]
                        panel::show(app, None);
                    }
                    "quit" => {
                        let handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = tokio::time::timeout(
                                std::time::Duration::from_secs(5),
                                handle.state::<TunnelState>().stop_all(),
                            )
                            .await;
                            handle.exit(0);
                        });
                    }
                    _ => {}
                })
                .build(app)?;
            #[cfg(target_os = "macos")]
            {
                let (press, begin, end, modal) = (
                    app.handle().clone(),
                    app.handle().clone(),
                    app.handle().clone(),
                    app.handle().clone(),
                );
                status_item::install(
                    &tray,
                    status_item::Handlers {
                        press: Box::new(move || panel::toggle(&press, panel::tray_anchor(&press))),
                        begin: Box::new(move |clicked| {
                            panel::present(&begin, panel::tray_anchor(&begin), !clicked)
                        }),
                        // The system may end the session while a dialog
                        // opened from the panel holds focus; it stays open.
                        end: Box::new(move || {
                            if !panel::is_modal(&end) {
                                panel::hide(&end)
                            }
                        }),
                        modal: Box::new(move || panel::is_modal(&modal)),
                    },
                );
            }
            panel::set_tray(app.handle(), tray);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            accounts,
            sync_remote_accounts,
            import_json,
            import_current,
            import_account_file,
            delete_account,
            switch_account,
            smart_switch,
            refresh_usage,
            next_full_refresh_at,
            say_hi,
            renew_now,
            save_account_proxy,
            set_renewal,
            greeting_schedule,
            set_greeting_schedule,
            proxy_status,
            start_proxy,
            stop_proxy,
            rotate_proxy_key,
            preferences,
            save_preferences,
            paths,
            quit_app,
            native_icons::native_symbols,
            configuration,
            provider_definitions,
            current_provider,
            providers,
            save_provider,
            remove_provider,
            refresh_provider,
            installed_editors,
            begin_login,
            finish_login,
            cancel_login,
            pick_identity_file,
            pick_settings_file,
            save_remote,
            remove_remote,
            remote_action,
            save_tunnel_configuration,
            tunnel_action,
            refresh_activity,
            cancel_workspace_refresh,
            workspace_directory,
            refresh_workspace_names,
            dismiss_workspace,
            fit_panel,
            hide_panel,
            navigate_menu_bar
        ])
        .build(tauri::generate_context!())
        .expect("Failed to build Copool");
    // An accessory app from launch, as the SwiftUI app's LSUIElement made it:
    // tao applies this policy before it creates the panel, which can join a
    // full-screen app's space only if created while the app is an accessory.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    let closing = std::sync::atomic::AtomicBool::new(false);
    app.run(move |handle, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } = &event
        {
            panel::show(handle, None);
        }
        if let tauri::RunEvent::ExitRequested {
            api, code: None, ..
        } = event
        {
            api.prevent_exit();
            if closing.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(login) = handle.state::<LoginState>().0.lock().await.take() {
                    login.cancel();
                }
                if let Some((_, task)) = handle
                    .state::<WorkspaceRefreshState>()
                    .0
                    .lock()
                    .await
                    .take()
                {
                    task.abort();
                }
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    handle.state::<TunnelState>().stop_all(),
                )
                .await;
                handle.exit(0);
            });
        }
    });
}
