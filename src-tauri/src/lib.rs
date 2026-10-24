// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

pub mod audio;
pub mod commands;
pub mod float_window;
pub mod hotkeys;
pub mod logging;

use tauri::Manager;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::Emitter;
use tauri_plugin_global_shortcut::ShortcutState;

/// 构建托盘菜单：共享开关文案随状态变化（TC-022 托盘快捷操作；TC-055 显示悬浮窗）
fn build_tray_menu(app: &tauri::AppHandle, master_on: bool) -> tauri::Result<Menu<tauri::Wry>> {
    let show = MenuItem::with_id(app, "show", "打开主界面", true, None::<&str>)?;
    let show_float = MenuItem::with_id(app, "show_float", "显示悬浮窗", true, None::<&str>)?;
    let toggle_share = MenuItem::with_id(
        app,
        "toggle_share",
        if master_on { "关闭共享" } else { "开启共享" },
        true,
        None::<&str>,
    )?;
    let media_play_pause = MenuItem::with_id(app, "media_play_pause", "播放/暂停音乐", true, None::<&str>)?;
    let check_update = MenuItem::with_id(app, "check_update", "检查更新", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    Menu::with_items(app, &[&show, &show_float, &toggle_share, &media_play_pause, &check_update, &quit])
}

/// 重建托盘菜单（总开关状态变化时同步文案；UI/热键/托盘三入口一致）
pub fn rebuild_tray_menu(app: &tauri::AppHandle, master_on: bool) {
    if let Some(tray) = app.tray_by_id("main-tray") {
        if let Ok(menu) = build_tray_menu(app, master_on) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

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
        // 全局热键（TC-018/019）：事件仅在按下时分发；注册由 hotkeys::register_all 管理
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        crate::hotkeys::handle(app, shortcut);
                    }
                })
                .build(),
        )
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

            // 托盘常驻（PRD·托盘常驻）：双击恢复窗口，菜单可开关共享/播放暂停/退出、检查更新
            let initial_master_on = commands::master_on();
            let menu = build_tray_menu(app.handle(), initial_master_on)?;
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
                        "show_float" => {
                            crate::float_window::show_float_window(app);
                        }
                        "toggle_share" => {
                            // 与热键共用动作分发（TC-022）
                            commands::toggle_master(app.clone(), !commands::master_on());
                        }
                        "media_play_pause" => {
                            crate::hotkeys::media_play_pause();
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
            // 启动注册全局热键（TC-018/019；单条失败仅记日志，不阻塞启动）
            crate::hotkeys::register_all(app.handle());
            // 第2期 P1：启动时自动切默认麦克风到 CABLE（失败仅记日志，不阻塞启动）
            commands::switch_default_mic_to_cable_on_startup(app.handle());
            // 第3期：设置开启过悬浮窗 → 启动时重建（位置越界 clamp 到当前工作区；失败仅记日志）
            let fw_cfg = crate::float_window::load_float_config(app.handle());
            if fw_cfg.enabled {
                if let Err(e) = crate::float_window::create_float_window(app.handle(), &fw_cfg, true) {
                    log::warn!(target: "float", "启动重建悬浮窗失败: {e}");
                }
            }
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
            commands::get_hotkeys,
            commands::set_hotkeys,
            commands::set_hotkeys_recording,
            commands::finish_onboarding,
            commands::install_cable,
            commands::get_remember,
            commands::set_remember,
            commands::write_log,
            commands::get_version,
            commands::get_app_icon,
            commands::get_favorites,
            commands::set_favorite,
            commands::media_play_pause,
            commands::quit_app,
            commands::set_output_mute,
            float_window::get_float_config,
            float_window::set_float_enabled,
            float_window::snap_float,
            float_window::save_float_position,
            float_window::show_main_window
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
