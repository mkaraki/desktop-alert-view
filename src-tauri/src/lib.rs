mod config;
mod models;
mod services;

use config::{get_config_path, load_or_create_config, load_window_state, save_window_state, WindowState};

use models::{AlertFetchResult, AlertItem};
use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager,
};
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
async fn fetch_all_alerts(app_handle: AppHandle) -> Result<AlertFetchResult, String> {
    let config = load_or_create_config(&app_handle)?;
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .danger_accept_invalid_certs(true) // Allow self-signed certs commonly used in internal monitoring
        .user_agent("DesktopAlertView/1.0")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let client = Arc::new(client);
    let mut active_alerts: Vec<AlertItem> = Vec::new();
    let mut closed_unacked_alerts: Vec<AlertItem> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // 1. Zabbix instances
    let mut zabbix_futures = Vec::new();
    for zb in config.zabbix {
        let client_clone = Arc::clone(&client);
        zabbix_futures.push(tokio::spawn(async move {
            let res = services::zabbix::fetch_zabbix_alerts(&client_clone, &zb).await;
            (zb.name, res)
        }));
    }

    // 2. Grafana instances
    let mut grafana_futures = Vec::new();
    for gf in config.grafana {
        let client_clone = Arc::clone(&client);
        grafana_futures.push(tokio::spawn(async move {
            let res = services::grafana::fetch_grafana_alerts(&client_clone, &gf).await;
            (gf.name, res)
        }));
    }

    // 3. Uptime Kuma instances
    let mut kuma_futures = Vec::new();
    for uk in config.uptime_kuma {
        let client_clone = Arc::clone(&client);
        kuma_futures.push(tokio::spawn(async move {
            let res = services::uptime_kuma::fetch_uptime_kuma_alerts(&client_clone, &uk).await;
            (uk.name, res)
        }));
    }

    // Await Zabbix results
    for fut in zabbix_futures {
        if let Ok((name, res)) = fut.await {
            match res {
                Ok((active, closed_unacked)) => {
                    active_alerts.extend(active);
                    closed_unacked_alerts.extend(closed_unacked);
                }
                Err(err) => {
                    errors.push(format!("Zabbix [{}]: {}", name, err));
                }
            }
        }
    }

    // Await Grafana results
    for fut in grafana_futures {
        if let Ok((name, res)) = fut.await {
            match res {
                Ok((active, closed_unacked)) => {
                    active_alerts.extend(active);
                    closed_unacked_alerts.extend(closed_unacked);
                }
                Err(err) => {
                    errors.push(format!("Grafana [{}]: {}", name, err));
                }
            }
        }
    }

    // Await Uptime Kuma results
    for fut in kuma_futures {
        if let Ok((name, res)) = fut.await {
            match res {
                Ok(alerts) => {
                    active_alerts.extend(alerts);
                }
                Err(err) => {
                    errors.push(format!("Uptime Kuma [{}]: {}", name, err));
                }
            }
        }
    }

    // Sort active alerts by severity / timestamp
    active_alerts.sort_by(|a, b| severity_rank(&a.severity).cmp(&severity_rank(&b.severity)));

    // Cap closed / resolved history alerts to max_resolved_history from config.yaml (default 100)
    if closed_unacked_alerts.len() > config.max_resolved_history {
        closed_unacked_alerts.truncate(config.max_resolved_history);
    }

    Ok(AlertFetchResult {
        active_alerts,
        closed_unacked_alerts,
        errors,
    })
}

fn severity_rank(sev: &str) -> u8 {
    match sev {
        "disaster" | "down" => 0,
        "high" => 1,
        "warning" => 2,
        "info" => 3,
        _ => 4,
    }
}

#[tauri::command]
fn start_dragging(window: tauri::Window) {
    let _ = window.start_dragging();
}

#[tauri::command]
fn get_config_file_path(app_handle: AppHandle) -> String {
    get_config_path(&app_handle).to_string_lossy().to_string()
}

#[tauri::command]
fn open_config_folder(app_handle: AppHandle) -> Result<(), String> {
    let path = get_config_path(&app_handle);
    let parent = path.parent().unwrap_or(&path);
    app_handle
        .opener()
        .open_path(parent.to_string_lossy().as_ref(), None::<&str>)
        .map_err(|e| format!("Failed to open config folder: {}", e))
}

#[tauri::command]
fn open_config_file(app_handle: AppHandle) -> Result<(), String> {
    let path = get_config_path(&app_handle);
    let _ = load_or_create_config(&app_handle);
    app_handle
        .opener()
        .open_path(path.to_string_lossy().as_ref(), None::<&str>)
        .map_err(|e| format!("Failed to open config file: {}", e))
}

#[tauri::command]
fn open_external_url(app_handle: AppHandle, url: String) -> Result<(), String> {
    app_handle
        .opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| format!("Failed to open URL {}: {}", url, e))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let _ = load_or_create_config(app.handle());

            let handle = app.handle().clone();
            if let Some(window) = app.get_webview_window("main") {
                if let Some(saved_state) = load_window_state(&handle) {
                    let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
                        x: saved_state.x,
                        y: saved_state.y,
                    }));
                    let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
                        width: saved_state.width,
                        height: saved_state.height,
                    }));
                }

                let window_clone = window.clone();
                let handle_clone = handle.clone();
                window.on_window_event(move |event| match event {
                    tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Moved(_) => {
                        if let (Ok(pos), Ok(size)) = (window_clone.outer_position(), window_clone.outer_size()) {
                            save_window_state(
                                &handle_clone,
                                &WindowState {
                                    x: pos.x,
                                    y: pos.y,
                                    width: size.width,
                                    height: size.height,
                                },
                            );
                        }
                    }
                    _ => {}
                });
            }

            let show_i = MenuItem::with_id(app, "show", "Show Widget", true, None::<&str>)?;
            let open_dir_i = MenuItem::with_id(app, "open_dir", "Open Config Folder", true, None::<&str>)?;
            let open_file_i = MenuItem::with_id(app, "open_file", "Open config.yaml", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

            let menu = Menu::with_items(app, &[&show_i, &open_dir_i, &open_file_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                    "open_dir" => {
                        let path = get_config_path(app);
                        let parent = path.parent().unwrap_or(&path);
                        let _ = app.opener().open_path(parent.to_string_lossy().as_ref(), None::<&str>);
                    }
                    "open_file" => {
                        let path = get_config_path(app);
                        let _ = app.opener().open_path(path.to_string_lossy().as_ref(), None::<&str>);
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            fetch_all_alerts,
            start_dragging,
            get_config_file_path,
            open_config_folder,
            open_config_file,
            open_external_url
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
