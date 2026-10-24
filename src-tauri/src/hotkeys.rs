//! 全局热键：配置解析、注册与动作分发（技术设计 §4 hotkeys.rs）
//! 追溯：TC-018（热键开关共享）、TC-019（媒体键）、TC-020（自定义改键）、TC-021（冲突回滚）
//! 热键格式："Ctrl+Shift+S"（修饰键顺序固定，主键白名单见 KEY_NAMES）；空串=禁用。
//! 注册全部在后端完成（tauri-plugin-global-shortcut），前端仅做按键捕获录制。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
};

/// 热键配置（hotkeys.json 结构；空串=禁用该热键）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HotkeyConfig {
    pub toggle_share: String,
    pub play_pause: String,
    pub prev_track: String,
    pub next_track: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            toggle_share: "Ctrl+Shift+S".into(),
            play_pause: "Ctrl+Shift+P".into(),
            prev_track: "Ctrl+Shift+Left".into(),
            next_track: "Ctrl+Shift+Right".into(),
        }
    }
}

/// VK_MEDIA_* 裸值（Windows 定义值，固定裸值避免 windows crate 版本差异）
const VK_MEDIA_PLAY_PAUSE: u16 = 0xB3;
const VK_MEDIA_PREV_TRACK: u16 = 0xB1;
const VK_MEDIA_NEXT_TRACK: u16 = 0xB0;

const HOTKEYS_FILE: &str = "hotkeys.json";

/// hotkeys.json 绝对路径：统一存于 %APPDATA%\AudioShare（与 remember.json 同目录约定）
pub fn hotkeys_path(app: &AppHandle) -> Option<PathBuf> {
    crate::data_dir(app).map(|d| d.join(HOTKEYS_FILE))
}

/// 读取热键配置；文件缺失/解析失败回退默认值（与 remember.json 容错策略一致）
pub fn load_config(app: &AppHandle) -> HotkeyConfig {
    let Some(p) = hotkeys_path(app) else {
        return HotkeyConfig::default();
    };
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return HotkeyConfig::default(); // 首次运行无配置，属正常
    };
    match serde_json::from_str(&raw) {
        Ok(c) => c,
        Err(e) => {
            log::warn!(target: "hotkey", "hotkeys.json 解析失败，使用默认配置: {e}");
            HotkeyConfig::default()
        }
    }
}

/// 覆盖写入热键配置
pub fn save_config(app: &AppHandle, cfg: &HotkeyConfig) {
    let Some(p) = hotkeys_path(app) else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(cfg)
        .map_err(|e| e.to_string())
        .and_then(|s| std::fs::write(&p, s).map_err(|e| e.to_string()))
    {
        Ok(()) => log::info!(target: "hotkey", "hotkeys.json 已覆盖写入"),
        Err(e) => log::error!(target: "hotkey", "hotkeys.json 写入失败: {e}"),
    }
}

/// 主键白名单：显示名 -> Code（parse 与 format 共用同一张表，保证往返一致）
const KEY_NAMES: &[(&str, Code)] = &[
    ("A", Code::KeyA), ("B", Code::KeyB), ("C", Code::KeyC), ("D", Code::KeyD),
    ("E", Code::KeyE), ("F", Code::KeyF), ("G", Code::KeyG), ("H", Code::KeyH),
    ("I", Code::KeyI), ("J", Code::KeyJ), ("K", Code::KeyK), ("L", Code::KeyL),
    ("M", Code::KeyM), ("N", Code::KeyN), ("O", Code::KeyO), ("P", Code::KeyP),
    ("Q", Code::KeyQ), ("R", Code::KeyR), ("S", Code::KeyS), ("T", Code::KeyT),
    ("U", Code::KeyU), ("V", Code::KeyV), ("W", Code::KeyW), ("X", Code::KeyX),
    ("Y", Code::KeyY), ("Z", Code::KeyZ),
    ("0", Code::Digit0), ("1", Code::Digit1), ("2", Code::Digit2), ("3", Code::Digit3),
    ("4", Code::Digit4), ("5", Code::Digit5), ("6", Code::Digit6), ("7", Code::Digit7),
    ("8", Code::Digit8), ("9", Code::Digit9),
    ("F1", Code::F1), ("F2", Code::F2), ("F3", Code::F3), ("F4", Code::F4),
    ("F5", Code::F5), ("F6", Code::F6), ("F7", Code::F7), ("F8", Code::F8),
    ("F9", Code::F9), ("F10", Code::F10), ("F11", Code::F11), ("F12", Code::F12),
    ("Left", Code::ArrowLeft), ("Right", Code::ArrowRight),
    ("Up", Code::ArrowUp), ("Down", Code::ArrowDown),
    ("Space", Code::Space), ("Enter", Code::Enter), ("Escape", Code::Escape),
    ("Tab", Code::Tab), ("Backspace", Code::Backspace), ("Delete", Code::Delete),
    ("Insert", Code::Insert), ("Home", Code::Home), ("End", Code::End),
    ("PageUp", Code::PageUp), ("PageDown", Code::PageDown),
    ("Comma", Code::Comma), ("Period", Code::Period),
];

fn code_from_name(name: &str) -> Option<Code> {
    let upper = name.to_ascii_uppercase();
    KEY_NAMES
        .iter()
        .find(|(n, _)| (*n).to_ascii_uppercase() == upper)
        .map(|(_, c)| *c)
}

fn name_from_code(code: Code) -> Option<&'static str> {
    KEY_NAMES.iter().find(|(_, c)| *c == code).map(|(n, _)| *n)
}

/// 解析组合串 "Ctrl+Shift+S" → Shortcut。
/// 规则：≥1 个修饰键 + 恰 1 个白名单主键；空串/非法输入 → None（视为禁用）。
pub fn parse_shortcut(s: &str) -> Option<Shortcut> {
    let mut mods = Modifiers::empty();
    let mut key: Option<Code> = None;
    for token in s.split('+') {
        let t = token.trim();
        if t.is_empty() {
            return None;
        }
        match t.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.insert(Modifiers::CONTROL),
            "shift" => mods.insert(Modifiers::SHIFT),
            "alt" | "option" => mods.insert(Modifiers::ALT),
            "win" | "super" | "meta" | "cmd" | "command" => mods.insert(Modifiers::SUPER),
            _ => {
                // 多个主键或未知键位 → 非法
                if key.is_some() {
                    return None;
                }
                key = code_from_name(t);
                if key.is_none() {
                    return None;
                }
            }
        }
    }
    let key = key?;
    if mods.is_empty() {
        return None; // 拒绝无修饰键的单键热键（防误触/抢占常用键）
    }
    Some(Shortcut::new(Some(mods), key))
}

/// 格式化 Shortcut → "Ctrl+Shift+S"（修饰键固定顺序 Ctrl+Shift+Alt+Win）。
pub fn format_shortcut(sc: &Shortcut) -> Option<String> {
    let key_name = name_from_code(sc.key)?;
    let mut parts: Vec<&str> = Vec::new();
    if sc.mods.contains(Modifiers::CONTROL) {
        parts.push("Ctrl");
    }
    if sc.mods.contains(Modifiers::SHIFT) {
        parts.push("Shift");
    }
    if sc.mods.contains(Modifiers::ALT) {
        parts.push("Alt");
    }
    if sc.mods.contains(Modifiers::SUPER) {
        parts.push("Win");
    }
    parts.push(key_name);
    Some(parts.join("+"))
}

/// 按当前配置重新注册全部热键（先清空，逐条注册，失败仅记日志不阻塞）。
/// 录制改键期间由 set_hotkeys_recording 先注销，本函数用于恢复/启动注册。
pub fn register_all(app: &AppHandle) {
    let _ = app.global_shortcut().unregister_all();
    let cfg = load_config(app);
    for (label, s) in [
        ("toggle_share", &cfg.toggle_share),
        ("play_pause", &cfg.play_pause),
        ("prev_track", &cfg.prev_track),
        ("next_track", &cfg.next_track),
    ] {
        let Some(sc) = parse_shortcut(s) else {
            continue; // 空串/非法 → 该条禁用
        };
        if let Err(e) = app.global_shortcut().register(sc) {
            log::warn!(target: "hotkey", "{label} 注册失败（可能被其它程序占用）: {s} {e}");
        }
    }
}

/// 比对某条配置串与按下的热键是否一致
fn matches_shortcut(cfg_str: &str, pressed: &Shortcut) -> bool {
    parse_shortcut(cfg_str).map(|s| s == *pressed).unwrap_or(false)
}

/// 全局热键事件分发：命中配置即执行对应动作（TC-018/TC-019）。
/// 主开关翻转统一走 commands::toggle_master（内部 emit 事件 + 重建托盘文案）。
pub fn handle(app: &AppHandle, shortcut: &Shortcut) {
    let cfg = load_config(app);
    if matches_shortcut(&cfg.toggle_share, shortcut) {
        crate::commands::toggle_master(app.clone(), !crate::commands::master_on());
    } else if matches_shortcut(&cfg.play_pause, shortcut) {
        media_play_pause();
    } else if matches_shortcut(&cfg.prev_track, shortcut) {
        media_prev_track();
    } else if matches_shortcut(&cfg.next_track, shortcut) {
        media_next_track();
    }
}

/// 模拟全局媒体键：按下 + 抬起（音乐软件作为后台应用响应）
fn send_media_key(vk: u16) {
    unsafe {
        keybd_event(vk as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(vk as u8, 0, KEYEVENTF_KEYUP, 0);
    }
}

/// 播放/暂停音乐（VK_MEDIA_PLAY_PAUSE）
pub fn media_play_pause() {
    send_media_key(VK_MEDIA_PLAY_PAUSE);
}

/// 上一首（VK_MEDIA_PREV_TRACK）
pub fn media_prev_track() {
    send_media_key(VK_MEDIA_PREV_TRACK);
}

/// 下一首（VK_MEDIA_NEXT_TRACK）
pub fn media_next_track() {
    send_media_key(VK_MEDIA_NEXT_TRACK);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(s: &str) -> Shortcut {
        parse_shortcut(s).unwrap_or_else(|| panic!("应可解析: {s}"))
    }

    /// TC-020：默认 4 条热键解析/格式化往返一致
    #[test]
    fn roundtrip_defaults() {
        for s in ["Ctrl+Shift+S", "Ctrl+Shift+P", "Ctrl+Shift+Left", "Ctrl+Shift+Right"] {
            assert_eq!(format_shortcut(&sc(s)).as_deref(), Some(s), "往返失败: {s}");
        }
    }

    /// TC-020：常见变体（三修饰键、Shift+Space、Alt+数字、小写修饰键）
    #[test]
    fn roundtrip_variants() {
        for (input, expect) in [
            ("Ctrl+Alt+Win+F5", "Ctrl+Alt+Win+F5"),
            ("Shift+Space", "Shift+Space"),
            ("Alt+1", "Alt+1"),
            ("ctrl+shift+s", "Ctrl+Shift+S"),
            ("Ctrl+Shift+Period", "Ctrl+Shift+Period"),
        ] {
            assert_eq!(format_shortcut(&sc(input)).as_deref(), Some(expect), "变体失败: {input}");
        }
    }

    /// TC-020：非法输入拒绝（空串=禁用、纯修饰键、纯主键、未知键、多主键）
    #[test]
    fn rejects_invalid() {
        for s in ["", "Ctrl", "S", "Ctrl+Shift", "Foo", "Ctrl+Foo", "Ctrl+Shift+S+X", "+S"] {
            assert!(parse_shortcut(s).is_none(), "应拒绝: {s:?}");
        }
    }

    /// 默认配置值（TC-020 恢复默认的依据）
    #[test]
    fn default_config_values() {
        let d = HotkeyConfig::default();
        assert_eq!(d.toggle_share, "Ctrl+Shift+S");
        assert_eq!(d.play_pause, "Ctrl+Shift+P");
        assert_eq!(d.prev_track, "Ctrl+Shift+Left");
        assert_eq!(d.next_track, "Ctrl+Shift+Right");
    }

    /// TC-019：媒体键裸值常量非零（Windows VK_MEDIA_* 定义）
    #[test]
    fn media_key_values_nonzero() {
        assert_eq!(VK_MEDIA_PLAY_PAUSE, 0xB3);
        assert_eq!(VK_MEDIA_PREV_TRACK, 0xB1);
        assert_eq!(VK_MEDIA_NEXT_TRACK, 0xB0);
    }

    /// 空串禁用语义：parse 返回 None，调用方据此跳过注册
    #[test]
    fn empty_string_means_disabled() {
        assert!(parse_shortcut("").is_none());
    }
}
