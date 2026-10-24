//! 悬浮窗窗口管理（第3期）：第二窗口（label="float"）、位置配置持久化、边缘吸附纯函数
//! 追溯：TC-045~048/050/055~057（PRD FR-001/009/010/011/013/015）
//! 坐标单位：全程物理像素（Tauri v2 window API 与 GetMonitorInfoW rcWork 均为物理像素）

use serde::{Deserialize, Serialize};
use tauri::utils::config::Color;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

/// 吸附阈值（物理像素，PRD FR-010/011：边缘 ≤20px 自动吸附）
pub const SNAP_THRESHOLD: f64 = 20.0;
/// 悬浮窗尺寸（物理像素，PRD FR-001）
pub const SIZE_HORIZONTAL: (f64, f64) = (232.0, 44.0);
pub const SIZE_VERTICAL: (f64, f64) = (44.0, 232.0);
/// ⠿ 拖动滑块中心距布局端部的偏移（物理像素，padding 6 + 滑块宽 28 / 2，两布局一致）
pub const SLIDER_AXIS_OFFSET: f64 = 20.0;
/// 配置文件名（%APPDATA%\AudioShare\）
const FLOAT_WINDOW_FILE: &str = "float_window.json";

/// 悬浮窗布局方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    Horizontal,
    Vertical,
}

/// orientation 字段缺失时的默认值（serde 专用，不能用 String 的 Default 空串）
fn default_orientation() -> String {
    "horizontal".into()
}

/// 悬浮窗配置（float_window.json；缺失/非法回默认：关闭+居中+水平）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FloatWindowConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default = "default_orientation")]
    pub orientation: String, // "horizontal" | "vertical"；空/非法=水平
}

impl Default for FloatWindowConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            x: 0.0,
            y: 0.0,
            orientation: "horizontal".into(),
        }
    }
}

/// 矩形（物理像素）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 吸附计算结果
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SnapResult {
    pub x: f64,
    pub y: f64,
    pub orientation: Orientation,
}

/// 解析布局方向："vertical"→Vertical，其余（含空串/非法）→Horizontal
pub fn parse_orientation(s: &str) -> Orientation {
    if s == "vertical" {
        Orientation::Vertical
    } else {
        Orientation::Horizontal
    }
}

/// 工作区居中坐标
pub fn default_center(work: Rect, w: f64, h: f64) -> (f64, f64) {
    ((work.w - w) / 2.0, (work.h - h) / 2.0)
}

/// 边缘吸附：四边距离最小值 ≤ threshold 时吸附该边（顶/底=水平，左/右=垂直）；
/// 超过阈值原样返回并保持传入方向。距离并列优先级：顶 > 底 > 左 > 右。
pub fn snap_to_edge(rect: Rect, work: Rect, threshold: f64, current: Orientation) -> SnapResult {
    let d_t = rect.y;
    let d_b = work.h - (rect.y + rect.h);
    let d_l = rect.x;
    let d_r = work.w - (rect.x + rect.w);
    let min = d_t.min(d_b).min(d_l).min(d_r);
    if min > threshold {
        return SnapResult { x: rect.x, y: rect.y, orientation: current };
    }
    // 分支顺序即并列优先级：顶 > 底 > 左 > 右；贴底/贴右必须用目标布局尺寸
    // （如垂直窗 232 高贴底 → 水平窗 44 高，若用 rect.h 会悬空 188px）
    if d_t == min {
        SnapResult { x: rect.x, y: 0.0, orientation: Orientation::Horizontal }
    } else if d_b == min {
        SnapResult { x: rect.x, y: work.h - SIZE_HORIZONTAL.1, orientation: Orientation::Horizontal }
    } else if d_l == min {
        SnapResult { x: 0.0, y: rect.y, orientation: Orientation::Vertical }
    } else {
        SnapResult { x: work.w - SIZE_VERTICAL.0, y: rect.y, orientation: Orientation::Vertical }
    }
}

/// 越界修正：把窗口位置收进工作区（分辨率变化/拔显示器自愈）
pub fn clamp_to_work_area(x: f64, y: f64, w: f64, h: f64, work: Rect) -> (f64, f64) {
    let cx = if w >= work.w { 0.0 } else { x.clamp(0.0, work.w - w) };
    let cy = if h >= work.h { 0.0 } else { y.clamp(0.0, work.h - h) };
    (cx, cy)
}

/// 布局切换时的滑块锚点补偿：以 ⠿ 拖动滑块为旋转中心，切换后滑块中心保持不动
/// （水平→垂直：贴边轴 x 不变，补偿 y；垂直→水平：贴边轴 y 不变，补偿 x）
pub fn anchor_adjust(rect: Rect, mut r: SnapResult, current: Orientation, work: Rect) -> SnapResult {
    if r.orientation == current {
        return r;
    }
    if r.orientation == Orientation::Vertical {
        // 水平滑块中心 y = rect.y + rect.h/2（垂直居中）；垂直布局滑块中心 = r.y + SLIDER_AXIS_OFFSET
        r.y = (rect.y + rect.h / 2.0 - SLIDER_AXIS_OFFSET).clamp(0.0, work.h - SIZE_VERTICAL.1);
    } else {
        // 垂直滑块中心 x = rect.x + rect.w/2（水平居中）；水平布局滑块中心 = r.x + SLIDER_AXIS_OFFSET
        r.x = (rect.x + rect.w / 2.0 - SLIDER_AXIS_OFFSET).clamp(0.0, work.w - SIZE_HORIZONTAL.0);
    }
    r
}

// ---------- 配置持久化（TDD-T3） ----------

/// 读取配置（纯 IO 可测）：文件缺失/JSON 非法 → 默认值
fn read_float_config_from(path: &std::path::Path) -> FloatWindowConfig {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str::<FloatWindowConfig>(&s).unwrap_or_default(),
        Err(_) => FloatWindowConfig::default(),
    }
}

/// 写入配置（纯 IO 可测）：自动创建父目录
fn write_float_config_to(path: &std::path::Path, cfg: &FloatWindowConfig) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    std::fs::write(path, json)
}

/// 读取悬浮窗配置（容错：文件缺失/解析失败回默认）
pub fn load_float_config(app: &AppHandle) -> FloatWindowConfig {
    let Some(dir) = crate::data_dir(app) else {
        return FloatWindowConfig::default();
    };
    read_float_config_from(&dir.join(FLOAT_WINDOW_FILE))
}

/// 覆盖写入悬浮窗配置（失败仅记日志，不阻断交互）
pub fn save_float_config(app: &AppHandle, cfg: &FloatWindowConfig) {
    let Some(dir) = crate::data_dir(app) else {
        return;
    };
    if let Err(e) = write_float_config_to(&dir.join(FLOAT_WINDOW_FILE), cfg) {
        log::error!(target: "app", "保存悬浮窗配置失败: {e}");
    }
}

// ---------- 工作区查询与窗口生命周期（窗口 API 不做单测，真机验收覆盖） ----------

/// 悬浮窗所在显示器工作区（GetMonitorInfoW rcWork，物理像素，避开任务栏）
pub fn work_area_of_float_window(app: &AppHandle) -> Option<Rect> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    unsafe {
        // 悬浮窗存在 → 其所在显示器；否则主窗口所在显示器
        let hwnd: Option<HWND> = app
            .get_webview_window("float")
            .or_else(|| app.get_webview_window("main"))
            .and_then(|w| w.hwnd().ok());
        let hwnd = hwnd?;
        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO::default();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut info).as_bool() {
            Some(Rect {
                x: info.rcWork.left as f64,
                y: info.rcWork.top as f64,
                w: (info.rcWork.right - info.rcWork.left) as f64,
                h: (info.rcWork.bottom - info.rcWork.top) as f64,
            })
        } else {
            None
        }
    }
}

/// 创建悬浮窗窗口（label="float"，无边框置顶透明、不入任务栏、不可缩放；物理像素）
/// 已存在时无操作；未保存过位置（x=y=0）居中，否则 clamp 到当前工作区
pub fn create_float_window(app: &AppHandle, cfg: &FloatWindowConfig, visible: bool) -> tauri::Result<()> {
    if app.get_webview_window("float").is_some() {
        return Ok(());
    }
    let vertical = parse_orientation(&cfg.orientation) == Orientation::Vertical;
    let (w, h) = if vertical { SIZE_VERTICAL } else { SIZE_HORIZONTAL };
    // builder 的 inner_size/position 为逻辑像素签名，按主窗口 scale 换算（目标尺寸为物理像素）
    let scale = app
        .get_webview_window("main")
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or(1.0);
    let mut builder = WebviewWindowBuilder::new(app, "float", WebviewUrl::App("index.html".into()))
        .title("AudioShare 悬浮窗")
        .decorations(false)
        .always_on_top(true)
        .transparent(true)
        // Windows 上 WebView2 透明窗口默认白色合成底，显式透明背景色避免白底矩形（TC-046）
        .background_color(Color(0, 0, 0, 0))
        .skip_taskbar(true)
        .resizable(false)
        .inner_size(w / scale, h / scale)
        .visible(visible);
    if cfg.x == 0.0 && cfg.y == 0.0 {
        builder = builder.center();
    } else {
        let pos = match work_area_of_float_window(app) {
            Some(work) => clamp_to_work_area(cfg.x, cfg.y, w, h, work),
            None => (cfg.x, cfg.y),
        };
        builder = builder.position(pos.0 / scale, pos.1 / scale);
    }
    builder.build()?;
    log::info!(target: "float", "悬浮窗已创建 visible={visible}");
    Ok(())
}

/// 销毁悬浮窗窗口（不存在时无操作）。
/// 注意：不再用于开关关闭——Tauri v2 中 destroy 后 manager 仍持有 stale handle，
/// 重新开启时误判"已存在"跳过重建，只得到无 WebView 的空壳窗口（内容不渲染、无交互）。
/// 开关关闭走 hide_float_window，本函数仅保留供将来显式释放用。
pub fn destroy_float_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("float") {
        let _ = w.destroy();
        log::info!(target: "float", "悬浮窗已销毁");
    }
}

/// 隐藏悬浮窗（TC-046 开关关闭与 ✕ 一致）：仅 hide 不销毁，避免 destroy→重建的 WebView 空壳问题。
/// 隐藏即停用：落盘 enabled=false 并通知前端（主界面开关同步）
pub fn hide_float_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("float") {
        let _ = w.hide();
        log::info!(target: "float", "悬浮窗已隐藏");
    }
    let mut cfg = load_float_config(app);
    if cfg.enabled {
        cfg.enabled = false;
        save_float_config(app, &cfg);
    }
    let _ = app.emit("float_enabled_changed", false);
}

/// 显示悬浮窗：不存在则按配置创建，存在则 show + 聚焦（托盘/设置开关共用）。
/// 显示即启用：落盘 enabled=true 并通知前端（主界面开关同步，托盘入口同样生效）
pub fn show_float_window(app: &AppHandle) {
    if app.get_webview_window("float").is_none() {
        let cfg = load_float_config(app);
        if let Err(e) = create_float_window(app, &cfg, true) {
            log::error!(target: "float", "创建悬浮窗失败: {e}");
            return;
        }
    }
    if let Some(w) = app.get_webview_window("float") {
        let _ = w.show();
        let _ = w.set_focus();
    }
    let mut cfg = load_float_config(app);
    if !cfg.enabled {
        cfg.enabled = true;
        save_float_config(app, &cfg);
    }
    let _ = app.emit("float_enabled_changed", true);
}

// ---------- 命令（前端 invoke） ----------

/// 读取悬浮窗配置（前端初始化）
#[tauri::command]
pub fn get_float_config(app: AppHandle) -> FloatWindowConfig {
    load_float_config(&app)
}

/// 开关悬浮窗：on=true 显示悬浮窗并自动隐藏主界面（第3期验收反馈），
/// on=false 隐藏悬浮窗；enabled 落盘与前端事件由 show/hide_float_window 统一处理
#[tauri::command]
pub fn set_float_enabled(app: AppHandle, on: bool) -> Result<(), String> {
    if on {
        show_float_window(&app);
        if let Some(m) = app.get_webview_window("main") {
            let _ = m.hide();
        }
    } else {
        hide_float_window(&app);
    }
    Ok(())
}

/// 边缘吸附（前端 onMoved 防抖 200ms 后调用）：当前方向按窗口尺寸推断（w>h=水平）；
/// 布局切换时以 ⠿ 滑块为锚点补偿另一轴（消除窗口跳位）
#[tauri::command]
pub fn snap_float(app: AppHandle, x: f64, y: f64, w: f64, h: f64) -> Result<SnapResult, String> {
    let work = work_area_of_float_window(&app).ok_or("无法获取显示器工作区")?;
    let rect = Rect { x, y, w, h };
    let current = if w > h { Orientation::Horizontal } else { Orientation::Vertical };
    let r = snap_to_edge(rect, work, SNAP_THRESHOLD, current);
    Ok(anchor_adjust(rect, r, current, work))
}

/// 保存位置与方向（拖动/吸附后落盘；不改变 enabled）
#[tauri::command]
pub fn save_float_position(app: AppHandle, x: f64, y: f64, orientation: String) {
    let mut cfg = load_float_config(&app);
    cfg.x = x;
    cfg.y = y;
    cfg.orientation = orientation;
    save_float_config(&app, &cfg);
}

/// 打开主界面（悬浮窗 🖥 按钮；与托盘 show 同逻辑）
#[tauri::command]
pub fn show_main_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> Rect {
        Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 }
    }

    /// TC-047：距顶 ≤ 阈值 → 贴顶，水平布局
    #[test]
    fn snap_to_top_edge_is_horizontal() {
        let r = snap_to_edge(Rect { x: 600.0, y: 10.0, w: 232.0, h: 44.0 }, work(), SNAP_THRESHOLD, Orientation::Horizontal);
        assert_eq!(r.x, 600.0);
        assert_eq!(r.y, 0.0);
        assert_eq!(r.orientation, Orientation::Horizontal);
    }

    /// TC-047：距底 ≤ 阈值 → 贴底，水平布局
    #[test]
    fn snap_to_bottom_edge_is_horizontal() {
        let work = work();
        let r = snap_to_edge(Rect { x: 600.0, y: work.h - 44.0 - 15.0, w: 232.0, h: 44.0 }, work, SNAP_THRESHOLD, Orientation::Horizontal);
        assert_eq!(r.y, work.h - 44.0);
        assert_eq!(r.orientation, Orientation::Horizontal);
    }

    /// TC-047/048：距左 ≤ 阈值 → 贴左，垂直布局
    #[test]
    fn snap_to_left_edge_is_vertical() {
        let r = snap_to_edge(Rect { x: 12.0, y: 400.0, w: 232.0, h: 44.0 }, work(), SNAP_THRESHOLD, Orientation::Horizontal);
        assert_eq!(r.x, 0.0);
        assert_eq!(r.y, 400.0);
        assert_eq!(r.orientation, Orientation::Vertical);
    }

    /// TC-047/048：距右 ≤ 阈值 → 贴右，垂直布局（用目标宽 44 而非当前宽 232）
    #[test]
    fn snap_to_right_edge_is_vertical() {
        let work = work();
        let r = snap_to_edge(Rect { x: work.w - 232.0 - 8.0, y: 400.0, w: 232.0, h: 44.0 }, work, SNAP_THRESHOLD, Orientation::Horizontal);
        assert_eq!(r.x, work.w - SIZE_VERTICAL.0);
        assert_eq!(r.orientation, Orientation::Vertical);
    }

    /// 验收反馈：垂直窗拖到右边缘 → 贴右且右边缘贴齐（切换后无悬空/错位）
    #[test]
    fn snap_to_bottom_from_vertical_uses_target_height() {
        let work = work();
        // 垂直窗（44×232）底部距屏幕底 8px → 贴底 → 水平窗（232×44）底部必须贴齐
        let r = snap_to_edge(Rect { x: 600.0, y: work.h - 232.0 - 8.0, w: 44.0, h: 232.0 }, work, SNAP_THRESHOLD, Orientation::Vertical);
        assert_eq!(r.orientation, Orientation::Horizontal);
        assert_eq!(r.y, work.h - SIZE_HORIZONTAL.1); // 996 = 1040 - 44
    }

    /// 阈值外不吸附：位置不变、方向保持传入值（含从垂直拖回中间）
    #[test]
    fn no_snap_when_far_from_edges() {
        let rect = Rect { x: 500.0, y: 500.0, w: 232.0, h: 44.0 };
        let r = snap_to_edge(rect, work(), SNAP_THRESHOLD, Orientation::Vertical);
        assert_eq!((r.x, r.y), (500.0, 500.0));
        assert_eq!(r.orientation, Orientation::Vertical); // 自由位置方向由调用方决定
    }

    /// 恰等于阈值 → 吸附（≤ 判定）
    #[test]
    fn snap_at_exact_threshold() {
        let r = snap_to_edge(Rect { x: SNAP_THRESHOLD, y: 400.0, w: 232.0, h: 44.0 }, work(), SNAP_THRESHOLD, Orientation::Horizontal);
        assert_eq!(r.x, 0.0);
        assert_eq!(r.orientation, Orientation::Vertical);
    }

    /// 多边并列优先级：顶 > 底 > 左 > 右（角部并列场景逐一验证）
    #[test]
    fn tie_break_priority_top_bottom_left_right() {
        let work = work();
        // 四边全贴（左上角 0,0）：dT=dL=0 并列 → 顶优先：水平贴顶
        let r = snap_to_edge(Rect { x: 0.0, y: 0.0, w: 232.0, h: 44.0 }, work, SNAP_THRESHOLD, Orientation::Horizontal);
        assert_eq!(r.orientation, Orientation::Horizontal);
        assert_eq!((r.x, r.y), (0.0, 0.0));
        // dT 与 dR 并列（右上角 8px）→ 顶优先：水平贴顶
        let r2 = snap_to_edge(Rect { x: work.w - 232.0 - 8.0, y: 8.0, w: 232.0, h: 44.0 }, work, SNAP_THRESHOLD, Orientation::Horizontal);
        assert_eq!(r2.orientation, Orientation::Horizontal);
        assert_eq!(r2.y, 0.0);
        // dB 与 dL 并列（左下角 8px）→ 底优先（底在左前）：水平贴底
        let r3 = snap_to_edge(Rect { x: 8.0, y: work.h - 44.0 - 8.0, w: 232.0, h: 44.0 }, work, SNAP_THRESHOLD, Orientation::Horizontal);
        assert_eq!(r3.orientation, Orientation::Horizontal);
        assert_eq!((r3.x, r3.y), (8.0, work.h - 44.0));
        // dB 与 dR 并列（右下角 6px）→ 底优先：水平贴底
        let r4 = snap_to_edge(Rect { x: work.w - 232.0 - 6.0, y: work.h - 44.0 - 6.0, w: 232.0, h: 44.0 }, work, SNAP_THRESHOLD, Orientation::Horizontal);
        assert_eq!(r4.orientation, Orientation::Horizontal);
        assert_eq!(r4.y, work.h - 44.0);
    }

    /// TC-057：越界四向均拉回工作区
    #[test]
    fn clamp_pulls_into_work_area() {
        let work = work();
        // 左上越界
        assert_eq!(clamp_to_work_area(-50.0, -30.0, 232.0, 44.0, work), (0.0, 0.0));
        // 右下越界
        assert_eq!(clamp_to_work_area(2000.0, 1200.0, 232.0, 44.0, work), (work.w - 232.0, work.h - 44.0));
        // 区内不动
        assert_eq!(clamp_to_work_area(600.0, 400.0, 232.0, 44.0, work), (600.0, 400.0));
        // 窗口大于工作区：贴左上
        assert_eq!(clamp_to_work_area(100.0, 100.0, 3000.0, 2000.0, work), (0.0, 0.0));
    }

    /// 默认居中：工作区几何中心
    #[test]
    fn center_is_work_area_midpoint() {
        let work = work();
        assert_eq!(default_center(work, 232.0, 44.0), ((work.w - 232.0) / 2.0, (work.h - 44.0) / 2.0));
    }

    /// 验收反馈：水平→垂直以 ⠿ 滑块为锚点，y 补偿使滑块中心不动（原滑块中心 y=422 → 新 y=402）
    #[test]
    fn anchor_adjust_h_to_v_keeps_slider_center() {
        let rect = Rect { x: 12.0, y: 400.0, w: 232.0, h: 44.0 };
        let r = snap_to_edge(rect, work(), SNAP_THRESHOLD, Orientation::Horizontal);
        let r = anchor_adjust(rect, r, Orientation::Horizontal, work());
        assert_eq!(r.orientation, Orientation::Vertical);
        assert_eq!(r.x, 0.0); // 贴边轴不变
        assert_eq!(r.y, 400.0 + 22.0 - SLIDER_AXIS_OFFSET); // 402.0
    }

    /// 验收反馈：垂直→水平以 ⠿ 滑块为锚点，x 补偿使滑块中心不动（垂直贴左拖到顶，滑块中心 x=522 → 新 x=502）
    #[test]
    fn anchor_adjust_v_to_h_keeps_slider_center() {
        let rect = Rect { x: 500.0, y: 5.0, w: 44.0, h: 232.0 };
        let r = snap_to_edge(rect, work(), SNAP_THRESHOLD, Orientation::Vertical);
        let r = anchor_adjust(rect, r, Orientation::Vertical, work());
        assert_eq!(r.orientation, Orientation::Horizontal);
        assert_eq!(r.y, 0.0); // 贴边轴不变
        assert_eq!(r.x, 500.0 + 22.0 - SLIDER_AXIS_OFFSET); // 502.0
    }

    /// 验收反馈：锚点补偿后越界 → 收进工作区（靠近底部切换垂直布局）
    #[test]
    fn anchor_adjust_clamps_into_work_area() {
        let work = work();
        // 距左 8px（距底 96px 超阈值）→ 左侧吸附 → 垂直；滑块中心 y=922 越界 → clamp 到底
        let rect2 = Rect { x: 8.0, y: 900.0, w: 232.0, h: 44.0 };
        let r2 = snap_to_edge(rect2, work, SNAP_THRESHOLD, Orientation::Horizontal);
        assert_eq!(r2.orientation, Orientation::Vertical);
        let r2 = anchor_adjust(rect2, r2, Orientation::Horizontal, work);
        assert_eq!(r2.x, 0.0);
        assert_eq!(r2.y, work.h - SIZE_VERTICAL.1); // clamp 到底部
    }

    /// 同方向不补偿：位置原样
    #[test]
    fn anchor_adjust_same_orientation_noop() {
        let rect = Rect { x: 600.0, y: 10.0, w: 232.0, h: 44.0 };
        let r = snap_to_edge(rect, work(), SNAP_THRESHOLD, Orientation::Horizontal);
        let r2 = anchor_adjust(rect, r, Orientation::Horizontal, work());
        assert_eq!((r2.x, r2.y), (r.x, r.y));
    }

    /// 验收反馈：贴右切换（水平→垂直）右边缘贴齐 + y 按滑块锚点补偿，无错位
    #[test]
    fn anchor_adjust_snap_right_no_misalignment() {
        let work = work();
        let rect = Rect { x: work.w - 232.0 - 8.0, y: 400.0, w: 232.0, h: 44.0 };
        let r = snap_to_edge(rect, work, SNAP_THRESHOLD, Orientation::Horizontal);
        let r = anchor_adjust(rect, r, Orientation::Horizontal, work);
        assert_eq!(r.x, work.w - SIZE_VERTICAL.0); // 贴右：右边缘 work.w
        assert_eq!(r.y, 400.0 + 22.0 - SLIDER_AXIS_OFFSET); // 滑块 y 中心不动
    }

    /// 验收反馈：贴底切换（垂直→水平）底边缘贴齐 + x 按滑块锚点补偿，无错位
    #[test]
    fn anchor_adjust_snap_bottom_no_misalignment() {
        let work = work();
        let rect = Rect { x: 600.0, y: work.h - 232.0 - 8.0, w: 44.0, h: 232.0 };
        let r = snap_to_edge(rect, work, SNAP_THRESHOLD, Orientation::Vertical);
        let r = anchor_adjust(rect, r, Orientation::Vertical, work);
        assert_eq!(r.y, work.h - SIZE_HORIZONTAL.1); // 贴底：底边缘 work.h
        assert_eq!(r.x, 600.0 + 22.0 - SLIDER_AXIS_OFFSET); // 滑块 x 中心不动
    }

    /// 方向解析：vertical→垂直；horizontal/空串/非法→水平
    #[test]
    fn parse_orientation_valid_and_fallback() {
        assert_eq!(parse_orientation("vertical"), Orientation::Vertical);
        assert_eq!(parse_orientation("horizontal"), Orientation::Horizontal);
        assert_eq!(parse_orientation(""), Orientation::Horizontal);
        assert_eq!(parse_orientation("diagonal"), Orientation::Horizontal);
    }

    // ---------- T3：配置持久化（纯 IO 测试） ----------

    fn temp_config_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("audioshare_fw_test_{}_{}.json", std::process::id(), name))
    }

    /// 文件缺失 → 默认配置（enabled=false）
    #[test]
    fn load_missing_file_returns_default() {
        let p = temp_config_path("missing");
        let _ = std::fs::remove_file(&p);
        let cfg = read_float_config_from(&p);
        assert!(!cfg.enabled);
        assert_eq!(cfg.orientation, "horizontal");
        assert_eq!((cfg.x, cfg.y), (0.0, 0.0));
    }

    /// JSON 非法 → 默认配置（不崩溃）
    #[test]
    fn load_invalid_json_returns_default() {
        let p = temp_config_path("invalid");
        std::fs::write(&p, "{ not valid json").unwrap();
        let cfg = read_float_config_from(&p);
        assert!(!cfg.enabled);
        let _ = std::fs::remove_file(&p);
    }

    /// 部分字段 JSON → 缺失字段回默认（serde default）
    #[test]
    fn load_partial_json_fills_defaults() {
        let p = temp_config_path("partial");
        std::fs::write(&p, r#"{"enabled":true}"#).unwrap();
        let cfg = read_float_config_from(&p);
        assert!(cfg.enabled);
        assert_eq!(cfg.orientation, "horizontal");
        assert_eq!((cfg.x, cfg.y), (0.0, 0.0));
        let _ = std::fs::remove_file(&p);
    }

    /// 写入→读回：字段完整往返（含非整数坐标与 vertical）
    #[test]
    fn save_then_load_roundtrip() {
        let p = temp_config_path("roundtrip");
        let cfg = FloatWindowConfig { enabled: true, x: 100.5, y: 200.25, orientation: "vertical".into() };
        write_float_config_to(&p, &cfg).unwrap();
        let loaded = read_float_config_from(&p);
        assert!(loaded.enabled);
        assert_eq!((loaded.x, loaded.y), (100.5, 200.25));
        assert_eq!(loaded.orientation, "vertical");
        assert_eq!(parse_orientation(&loaded.orientation), Orientation::Vertical);
        let _ = std::fs::remove_file(&p);
    }

    /// 父目录不存在 → 自动创建（多级）
    #[test]
    fn save_creates_missing_parent_dir() {
        let dir = std::env::temp_dir().join(format!("audioshare_fw_test_dir_{}", std::process::id()));
        let p = dir.join("nested").join("float_window.json");
        let _ = std::fs::remove_dir_all(&dir);
        write_float_config_to(&p, &FloatWindowConfig::default()).unwrap();
        assert!(p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
