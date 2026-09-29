// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

pub mod audio;
pub mod commands;
pub mod logging;

use tauri::Manager;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::Emitter;

/// 统一数据目录名：%APPDATA%\AudioShare（与 identifier 解耦，remember/settings/logs 均存于此）
pub const DATA_DIR_NAME: &str = "AudioShare";

/// 应用数据目录（不随 identifier 变化）
pub fn data_dir<R: tauri::Runtime>(m: &impl Manager<R>) -> Option<std::path::PathBuf> {
    m.path().config_dir().ok().map(|d| d.join(DATA_DIR_NAME))
}

/// 一次性迁移：旧版数据目录（随 identifier 命名）整体重命名为新目录，保留用户配置与日志
fn migrate_legacy_data_dir<R: tauri::Runtime>(m: &impl Manager<R>) {
    let Ok(cfg) = m.path().config_dir() else { return };
    let new_dir = cfg.join(DATA_DIR_NAME);
    if new_dir.exists() {
        return;
    }
    for legacy in ["com.xuhaoliang.audioshare", "com.xryon.audioshare"] {
        let old = cfg.join(legacy);
        if old.exists() {
            let _ = std::fs::rename(&old, &new_dir);
            break;
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // 日志体系先行：后续所有异常（含引擎初始化）均可落盘
            migrate_legacy_data_dir(app.handle());
            if let Some(dir) = data_dir(app.handle()) {
                if let Err(e) = logging::init(&dir) {
                    eprintln!("日志初始化失败，降级为无日志运行: {e}");
                }
            }
            log::info!(target: "app", "AudioShare 启动");

            // 初始化全局音频引擎状态
            commands::init_engine(app.handle().clone());

            // 托盘常驻（PRD·托盘常驻）：双击恢复窗口，菜单可退出、检查更新
            let show = MenuItem::with_id(app, "show", "打开主界面", true, None::<&str>)?;
            let check_update = MenuItem::with_id(app, "check_update", "检查更新", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &check_update, &quit])?;
            let _tray = TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("AudioShare")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    match event.id.as_ref() {
                        "show" => {
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                        "check_update" => {
                            // 通知前端执行更新检查
                            let _ = app.emit("tray-check-update", ());
                        }
                        "quit" => {
                            let _ = app.emit("tray-quit", ());
                            commands::quit_app(app.clone());
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::DoubleClick { .. } = event {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // 关闭按钮 → 隐藏到托盘而非退出（PRD·托盘常驻）
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                window.hide().ok();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            commands::get_apps,
            commands::get_mics,
            commands::check_default_mic,
            commands::get_status,
            commands::toggle_share,
            commands::set_volume,
            commands::set_mic,
            commands::toggle_master,
            commands::finish_onboarding,
            commands::install_cable,
            commands::get_remember,
            commands::set_remember,
            commands::write_log,
            commands::get_version,
            commands::get_app_icon,
            commands::quit_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
