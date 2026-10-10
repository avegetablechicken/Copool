#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use copool_core::{AccountSummary, ApiProxyStatus, Application, Paths, Preferences};
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, State,
};
mod appearance;
mod native_icons;
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
async fn import_refresh_token(app: State<'_, App>, token: String) -> Result<usize, String> {
    let result = app.import_refresh_token(&token).await?;
    Ok(result)
}
#[tauri::command]
async fn import_current(app: State<'_, App>) -> Result<usize, String> {
    let result = app.import_current().await?;
    Ok(result)
}
#[tauri::command]
async fn delete_account(app: State<'_, App>, id: String) -> Result<(), String> {
    let result = app.delete_account(&id).await?;
    Ok(result)
}
#[tauri::command]
async fn switch_account(app: State<'_, App>, id: String) -> Result<(), String> {
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
    match id {
        Some(id) => app.refresh_usage(&id).await,
        None => app.refresh_all().await,
    }
}
#[tauri::command]
async fn smart_switch(app: State<'_, App>) -> Result<String, String> {
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
    app.refresh_provider(&id, sync).await
}
#[tauri::command]
fn installed_editors() -> Vec<copool_core::platform::Editor> {
    copool_core::platform::editors()
}
#[derive(Default)]
struct NativeMenuState(std::sync::Mutex<Option<Menu<tauri::Wry>>>);
#[tauri::command]
fn account_menu(
    window: tauri::WebviewWindow,
    state: State<'_, NativeMenuState>,
    id: String,
    title: String,
    x: f64,
    y: f64,
) -> Result<(), String> {
    use tauri::menu::ContextMenu;
    if id.is_empty() || !x.is_finite() || !y.is_finite() {
        return Err("Invalid menu".into());
    }
    let item = MenuItem::with_id(
        window.app_handle(),
        format!("account-proxy:{id}"),
        title,
        true,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    let menu = Menu::with_items(window.app_handle(), &[&item]).map_err(|e| e.to_string())?;
    *state.0.lock().unwrap() = Some(menu.clone());
    menu.popup_at(
        window.as_ref().window().clone(),
        tauri::LogicalPosition::new(x, y),
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
fn import_menu(
    window: tauri::WebviewWindow,
    state: State<'_, NativeMenuState>,
    labels: Vec<String>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    use tauri::menu::ContextMenu;
    if labels.len() != 3 || !x.is_finite() || !y.is_finite() {
        return Err("Invalid menu".into());
    }
    let app = window.app_handle();
    let items = ["import-current", "import-file-open", "import-providers"]
        .into_iter()
        .zip(labels)
        .map(|(id, label)| MenuItem::with_id(app, id, label, true, None::<&str>))
        .collect::<tauri::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let menu =
        Menu::with_items(app, &[&items[0], &items[1], &items[2]]).map_err(|e| e.to_string())?;
    *state.0.lock().unwrap() = Some(menu.clone());
    menu.popup_at(
        window.as_ref().window().clone(),
        tauri::LogicalPosition::new(x, y),
    )
    .map_err(|e| e.to_string())
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
        show(&handle);
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
#[tauri::command]
async fn pick_identity_file(handle: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle.dialog().file().pick_file(move |file| {
        let _ = tx.send(file);
    });
    rx.await
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
) -> Result<usize, String> {
    use tauri_plugin_dialog::DialogExt;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    handle
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .pick_file(move |file| {
            let _ = sender.send(file);
        });
    let Some(file) = receiver.await.map_err(|e| e.to_string())? else {
        return Ok(0);
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 16 * 1024 * 1024 {
        return Err("File exceeds 16 MiB".into());
    }
    let count = app
        .import_json(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
        .await?;
    Ok(count)
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
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tunnel.0.lock().await.stop(),
    )
    .await;
    app.exit(0);
    Ok(())
}
fn show(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
fn main() {
    let app = tauri::Builder::default()
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .manage(NativeMenuState::default())
        .manage(LoginState::default())
        .manage(TunnelState::default())
        .manage(WorkspaceRefreshState::default())
        .plugin(tauri_plugin_dialog::init())
        .on_menu_event(|app, event| {
            if matches!(
                event.id.as_ref(),
                "import-current" | "import-file-open" | "import-providers"
            ) {
                let _ = app.emit("import-action", event.id.as_ref());
            }
            if let Some(id) = event.id.as_ref().strip_prefix("account-proxy:") {
                let _ = app.emit("account-proxy-menu", id);
            }
        })
        .plugin(tauri_plugin_single_instance::init(|app, _, _| show(app)))
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
            TrayIconBuilder::new()
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
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false)
                                && window.is_focused().unwrap_or(false)
                            {
                                let _ = window.hide();
                                return;
                            }
                            if let Ok(Some(monitor)) = window.current_monitor() {
                                let scale = monitor.scale_factor();
                                let anchor = rect.position.to_logical::<f64>(scale);
                                let size = rect.size.to_logical::<f64>(scale);
                                let origin = monitor.position().to_logical::<f64>(scale);
                                let screen = monitor.size().to_logical::<f64>(scale);
                                let x = (anchor.x + size.width / 2.0 - 271.0).clamp(
                                    origin.x,
                                    (origin.x + screen.width - 542.0).max(origin.x),
                                );
                                let y = (anchor.y + size.height + 4.0).clamp(
                                    origin.y,
                                    (origin.y + screen.height - 620.0).max(origin.y),
                                );
                                let _ = window.set_position(tauri::LogicalPosition::new(x, y));
                            }
                            show(app);
                        }
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show(app),
                    "quit" => {
                        let handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = handle.state::<TunnelState>().0.lock().await.stop().await;
                            handle.exit(0);
                        });
                    }
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            accounts,
            sync_remote_accounts,
            import_json,
            import_current,
            import_refresh_token,
            delete_account,
            switch_account,
            update_account,
            refresh_usage,
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
            configuration,
            provider_definitions,
            current_provider,
            save_provider,
            remove_provider,
            refresh_provider,
            import_menu,
            account_menu,
            installed_editors,
            begin_login,
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
            dismiss_workspace
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
            show(handle);
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
                    handle.state::<TunnelState>().0.lock().await.stop(),
                )
                .await;
                handle.exit(0);
            });
        }
    });
}
