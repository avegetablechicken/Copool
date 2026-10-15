#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use copool_core::{
    AccountSummary, ApiProxyStatus, Application, Paths, Preferences, SmartSwitchOutcome,
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
mod native_icons;
mod panel;
mod placement;
mod widget_support;
type App = Arc<Application>;

fn sync_remote_after_mutation(handle: tauri::AppHandle, app: App) {
    tauri::async_runtime::spawn(async move {
        let Ok(settings) = app.configuration() else {
            return;
        };
        let mut failures = vec![];
        for server in settings.remote_servers {
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
async fn sync_remote_accounts(handle: tauri::AppHandle, app: State<'_, App>) -> Result<(), String> {
    sync_remote_after_mutation(handle, app.inner().clone());
    Ok(())
}
#[tauri::command]
async fn accounts(app: State<'_, App>) -> Result<Vec<AccountSummary>, String> {
    app.accounts().await
}
#[tauri::command]
async fn import_json(app: State<'_, App>, content: String) -> Result<usize, String> {
    let result = app.import_json(&content).await?;
    Ok(result)
}
#[tauri::command]
async fn import_current(app: State<'_, App>) -> Result<usize, String> {
    let result = app.import_current().await?;
    Ok(result)
}
#[tauri::command]
async fn delete_account(app: State<'_, App>, id: String) -> Result<(), String> {
    app.delete_account(&id).await
}
#[tauri::command]
async fn switch_account(app: State<'_, App>, id: String) -> Result<SwitchExecution, String> {
    app.switch_account(&id).await
}
#[tauri::command]
async fn update_account(
    app: State<'_, App>,
    id: String,
    label: String,
    proxy_url: String,
) -> Result<(), String> {
    app.update_account(&id, &label, &proxy_url).await
}
#[tauri::command]
async fn refresh_usage(app: State<'_, App>, id: Option<String>) -> Result<(), String> {
    // A manual refresh may ask for Copool's Keychain item, which holds the
    // providers' passwords, when a request was refused before.
    let _ = app.authorize_keychain().await;
    // A manual refresh is what lets a new account contact upstream.
    app.release_new_accounts(id.as_deref()).await?;
    match id {
        Some(id) => app.refresh_usage(&id).await,
        None => app.refresh_all().await,
    }
}
/// The next background refresh of the whole pool, for both pools.
#[tauri::command]
fn next_full_refresh_at(app: State<'_, App>) -> Option<i64> {
    app.next_full_refresh_at()
}
#[tauri::command]
async fn smart_switch(app: State<'_, App>) -> Result<SmartSwitchOutcome, String> {
    app.smart_switch().await
}
#[tauri::command]
async fn proxy_status(app: State<'_, App>) -> Result<ApiProxyStatus, String> {
    app.proxy_status().await
}
#[tauri::command]
async fn start_proxy(app: State<'_, App>, port: Option<u16>) -> Result<ApiProxyStatus, String> {
    if let Some(port) = port {
        let mut preferences = app.preferences()?;
        preferences.proxy_port = port;
        app.save_preferences(preferences).await?;
    }
    app.start_proxy().await
}
#[tauri::command]
async fn stop_proxy(
    app: State<'_, App>,
    tunnel: State<'_, TunnelState>,
) -> Result<ApiProxyStatus, String> {
    tunnel.0.lock().await.stop().await?;
    app.stop_proxy().await
}
#[tauri::command]
async fn rotate_proxy_key(app: State<'_, App>) -> Result<ApiProxyStatus, String> {
    app.rotate_proxy_key().await
}
#[tauri::command]
fn preferences(app: State<'_, App>) -> Result<Preferences, String> {
    app.preferences()
}
#[tauri::command]
async fn save_preferences(app: State<'_, App>, value: Preferences) -> Result<(), String> {
    let old = app.preferences()?;
    if old.launch_at_startup != value.launch_at_startup {
        copool_core::platform::startup(
            value.launch_at_startup,
            &std::env::current_exe().map_err(|e| e.to_string())?,
        )?;
    }
    app.save_preferences(value).await
}
#[tauri::command]
fn paths(app: State<'_, App>) -> Paths {
    app.paths.clone()
}
#[tauri::command]
async fn save_account_proxy(app: State<'_, App>, id: String, value: String) -> Result<(), String> {
    app.save_account_proxy(&id, &value).await
}
#[tauri::command]
async fn set_renewal(app: State<'_, App>, id: String, renewal: String) -> Result<(), String> {
    app.set_renewal(&id, &renewal).await
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
#[tauri::command]
async fn save_provider(
    app: State<'_, App>,
    value: copool_core::settings::Provider,
) -> Result<copool_core::settings::Provider, String> {
    app.save_provider(value).await
}
#[tauri::command]
async fn remove_provider(app: State<'_, App>, id: String) -> Result<(), String> {
    app.remove_provider(&id).await
}
#[tauri::command]
async fn refresh_provider(app: State<'_, App>, id: String, sync: bool) -> Result<usize, String> {
    let _ = app.authorize_keychain().await;
    app.refresh_provider(&id, sync).await
}
#[tauri::command]
fn installed_editors() -> Vec<copool_core::platform::Editor> {
    copool_core::platform::editors()
}
#[derive(Default)]
struct LoginState(tokio::sync::Mutex<Option<Arc<copool_core::login::Login>>>);
#[tauri::command]
async fn begin_login(
    state: State<'_, LoginState>,
    workspace: Option<String>,
) -> Result<String, String> {
    let mut current = state.0.lock().await;
    if current.is_some() {
        return Err("Login already in progress".into());
    }
    let login = copool_core::login::Login::begin(workspace).await?;
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
        result=async {
            let document=login.finish().await?;
            if login.is_claude() {
                return app.claude_import_login(document).await.map(|_| 1);
            }
            let consent=document.get("consentWorkspaces").and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
            app.import_authorized(document,login.workspace(),&consent).await
        }=>result,
        _=login.cancelled()=>Err("error.oauth.request_cancelled".into())
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
/// A Claude Code sign-in; finish_login and cancel_login complete it.
#[tauri::command]
async fn begin_claude_login(state: State<'_, LoginState>) -> Result<String, String> {
    let mut current = state.0.lock().await;
    if current.is_some() {
        return Err("Login already in progress".into());
    }
    let login = copool_core::login::Login::begin_claude().await?;
    copool_core::platform::open_url(&login.url)?;
    let id = login.id.clone();
    *current = Some(login);
    Ok(id)
}
#[tauri::command]
async fn claude_accounts(app: State<'_, App>) -> Result<Vec<AccountSummary>, String> {
    app.claude_accounts().await
}
#[tauri::command]
async fn claude_import_current(app: State<'_, App>) -> Result<usize, String> {
    app.claude_import_current().await.map(|_| 1)
}
#[tauri::command]
async fn claude_delete_account(app: State<'_, App>, id: String) -> Result<(), String> {
    app.claude_delete_account(&id).await
}
#[tauri::command]
async fn claude_switch_account(app: State<'_, App>, id: String) -> Result<SwitchExecution, String> {
    app.claude_switch_account(&id).await
}
#[tauri::command]
async fn claude_smart_switch(app: State<'_, App>) -> Result<SmartSwitchOutcome, String> {
    app.claude_smart_switch().await
}
#[tauri::command]
async fn claude_refresh_usage(app: State<'_, App>, id: Option<String>) -> Result<(), String> {
    // A manual refresh may ask for the pool's Keychain item a startup missed.
    let _ = app.claude_authorize().await;
    app.claude_release_new_accounts(id.as_deref()).await?;
    match id {
        Some(id) => app.claude_refresh_usage(&id).await,
        None => app.claude_refresh(None).await,
    }
}
#[tauri::command]
async fn claude_save_account_proxy(
    app: State<'_, App>,
    id: String,
    value: String,
) -> Result<(), String> {
    app.claude_save_account_proxy(&id, &value).await
}
#[tauri::command]
async fn claude_set_renewal(
    app: State<'_, App>,
    id: String,
    renewal: String,
) -> Result<(), String> {
    app.claude_set_renewal(&id, &renewal).await
}
#[tauri::command]
fn claude_sub2api_providers(
    app: State<'_, App>,
) -> Result<Vec<copool_core::claude::Sub2ApiProvider>, String> {
    app.claude_sub2api_providers()
}
#[tauri::command]
async fn claude_save_sub2api_provider(
    app: State<'_, App>,
    value: copool_core::claude::Sub2ApiProvider,
) -> Result<copool_core::claude::Sub2ApiProvider, String> {
    app.claude_save_sub2api_provider(value).await
}
#[tauri::command]
async fn claude_remove_sub2api_provider(app: State<'_, App>, id: String) -> Result<(), String> {
    app.claude_remove_sub2api_provider(&id).await
}
#[tauri::command]
async fn claude_refresh_provider(app: State<'_, App>, id: String) -> Result<(), String> {
    let _ = app.authorize_keychain().await;
    app.claude_refresh_sub2api_provider(&id).await
}
#[tauri::command]
async fn claude_sync_sub2api(app: State<'_, App>) -> Result<usize, String> {
    let _ = app.authorize_keychain().await;
    app.claude_sync_sub2api().await
}
#[tauri::command]
async fn cancel_login(state: State<'_, LoginState>) -> Result<(), String> {
    if let Some(login) = state.0.lock().await.take() {
        login.cancel();
    }
    Ok(())
}
#[tauri::command]
async fn pick_identity_file(handle: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    panel::set_modal(&handle, true);
    handle.dialog().file().pick_file(move |file| {
        let _ = tx.send(file);
    });
    let picked = rx.await;
    panel::set_modal(&handle, false);
    picked
        .map_err(|e| e.to_string())?
        .map(|file| {
            file.into_path()
                .map(|p| p.to_string_lossy().into_owned())
                .map_err(|e| e.to_string())
        })
        .transpose()
}
/// A provider settings file, chosen from the Claude configuration directory.
#[tauri::command]
async fn pick_settings_file(
    handle: tauri::AppHandle,
    app: State<'_, App>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    panel::set_modal(&handle, true);
    handle
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_directory(app.claude_location().config_dir)
        .pick_file(move |file| {
            let _ = tx.send(file);
        });
    let picked = rx.await;
    panel::set_modal(&handle, false);
    picked
        .map_err(|e| e.to_string())?
        .map(|file| {
            file.into_path()
                .map(|p| p.to_string_lossy().into_owned())
                .map_err(|e| e.to_string())
        })
        .transpose()
}
#[tauri::command]
async fn import_account_file(
    handle: tauri::AppHandle,
    app: State<'_, App>,
) -> Result<Option<usize>, String> {
    import_account_file_for_pool(handle, app, false).await
}
#[tauri::command]
async fn claude_import_account_file(
    handle: tauri::AppHandle,
    app: State<'_, App>,
) -> Result<Option<usize>, String> {
    import_account_file_for_pool(handle, app, true).await
}
async fn import_account_file_for_pool(
    handle: tauri::AppHandle,
    app: State<'_, App>,
    claude: bool,
) -> Result<Option<usize>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    panel::set_modal(&handle, true);
    handle
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .pick_file(move |file| {
            let _ = sender.send(file);
        });
    let picked = receiver.await;
    panel::set_modal(&handle, false);
    // A cancelled picker imports nothing and reports nothing, as in Swift.
    let Some(file) = picked.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 16 * 1024 * 1024 {
        return Err("File exceeds 16 MiB".into());
    }
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let count = if claude {
        app.claude_import_json(&content).await?
    } else {
        app.import_json(&content).await?
    };
    Ok(Some(count))
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
#[derive(Default)]
struct TunnelState(tokio::sync::Mutex<copool_core::tunnel::Tunnel>);
#[tauri::command]
async fn save_tunnel_configuration(
    app: State<'_, App>,
    value: copool_core::tunnel::Configuration,
) -> Result<(), String> {
    app.save_tunnel_configuration(&value).await
}
#[tauri::command]
async fn tunnel_action(
    app: State<'_, App>,
    state: State<'_, TunnelState>,
    action: String,
    value: Option<copool_core::tunnel::Configuration>,
) -> Result<copool_core::tunnel::Status, String> {
    let mut tunnel = state.0.lock().await;
    match action.as_str() {
        "install" => copool_core::tunnel::install(&app.paths.data_dir).await?,
        "start" => {
            let proxy = app.proxy_status().await?;
            if !proxy.running {
                return Err("error.cloudflared.start_api_proxy_first".into());
            }
            let config = value.ok_or("Tunnel configuration missing")?;
            if !config.enabled {
                return Err("Enable public access first".into());
            }
            app.save_tunnel_configuration(&config).await?;
            let _ = app.authorize_keychain().await;
            let config = app.with_tunnel_token(config)?;
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
async fn quit_app(app: tauri::AppHandle, tunnel: State<'_, TunnelState>) -> Result<(), String> {
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tunnel.0.lock().await.stop().await
    })
    .await;
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
fn main() {
    #[cfg(target_os = "linux")]
    appearance::enable_compositing();
    let app = tauri::Builder::default()
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
            let service =
                Arc::new(Application::open(Paths::discover()?).map_err(std::io::Error::other)?);
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
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        rect,
                        position,
                        ..
                    } = event
                    {
                        panel::toggle(tray.app_handle(), Some(panel::anchor(rect, position)));
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        // GNOME fades the tray menu out (150 ms) after activation;
                        // waiting keeps it out of the panel's captured backdrop.
                        #[cfg(target_os = "linux")]
                        {
                            let handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                                let app = handle.clone();
                                let _ = handle.run_on_main_thread(move || panel::show(&app, None));
                            });
                        }
                        #[cfg(not(target_os = "linux"))]
                        panel::show(app, None);
                    }
                    "quit" => {
                        let handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ =
                                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                                    handle.state::<TunnelState>().0.lock().await.stop().await
                                })
                                .await;
                            handle.exit(0);
                        });
                    }
                    _ => {}
                })
                .build(app)?;
            panel::set_tray(app.handle(), tray);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            accounts,
            sync_remote_accounts,
            import_json,
            import_current,
            delete_account,
            switch_account,
            update_account,
            refresh_usage,
            next_full_refresh_at,
            smart_switch,
            proxy_status,
            start_proxy,
            stop_proxy,
            rotate_proxy_key,
            preferences,
            save_preferences,
            paths,
            quit_app,
            native_icons::native_symbols,
            save_account_proxy,
            set_renewal,
            configuration,
            provider_definitions,
            current_provider,
            save_provider,
            remove_provider,
            refresh_provider,
            installed_editors,
            begin_login,
            begin_claude_login,
            finish_login,
            cancel_login,
            import_account_file,
            pick_identity_file,
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
            claude_accounts,
            claude_import_current,
            claude_import_account_file,
            claude_delete_account,
            claude_switch_account,
            claude_smart_switch,
            claude_refresh_usage,
            claude_save_account_proxy,
            claude_set_renewal,
            claude_sub2api_providers,
            claude_save_sub2api_provider,
            claude_remove_sub2api_provider,
            claude_refresh_provider,
            pick_settings_file,
            claude_sync_sub2api
        ])
        .build(tauri::generate_context!())
        .expect("Failed to build Copool");
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
                let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    handle.state::<TunnelState>().0.lock().await.stop().await
                })
                .await;
                handle.exit(0);
            });
        }
    });
}
