//! 悬浮窗窗口管理（第3期）：第二窗口（label="float"）、位置配置持久化、边缘吸附纯函数
//! 追溯：TC-045~048/050/055~057（PRD FR-001/009/010/011/013/015）
//! 坐标单位：全程物理像素（Tauri v2 window API 与 GetMonitorInfoW rcWork 均为物理像素）

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::utils::config::Color;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

/// 主线程 ID（setup 阶段登记）
///
/// 【线程约束·关键】Tauri v2 运行期（event loop 已启动）在主线程调用
/// `WebviewWindowBuilder::build()` 会自等待死锁：build 内部向主线程 event loop
/// 投递创建事件并同步等待结果，而主线程在等待期间无法处理该事件。
/// 症状：开启悬浮窗后只出现无内容的空壳窗口、后续命令全部排队、
/// 日志不再输出、进程无法退出。setup 阶段建窗不受影响（event loop 尚未启动）。
static MAIN_THREAD_ID: OnceLock<std::thread::ThreadId> = OnceLock::new();

/// 登记主线程 ID（必须在 setup 中、event loop 启动前调用）
pub fn init_main_thread() {
    let _ = MAIN_THREAD_ID.set(std::thread::current().id());
}

/// 当前是否运行在主线程
fn is_main_thread() -> bool {
    MAIN_THREAD_ID.get() == Some(&std::thread::current().id())
}

/// 吸附阈值（物理像素，PRD FR-010/011：边缘 ≤20px 自动吸附）
pub const SNAP_THRESHOLD: f64 = 20.0;

/// 应用图标槽位（物理像素）：单个图标 20px，相邻图标间隔 4px
pub const ICON_SIZE: f64 = 20.0;
pub const ICON_GAP: f64 = 4.0;

/// 悬浮窗【基础尺寸】（物理像素，不含应用图标区），由《悬浮窗美化方案》3.4 控件规格推导
///
/// 水平（宽 = 边框 2 + padding 16 + 手柄 16 + gap 4 + 旋转 24(+mr 4) + gap 4
///        + 开关 30(+mr 6) + gap 4 + 图标区(mr 6) + gap 4 + 功能组 111 = 231 + 图标区宽）：
/// 高度固定 44（2 边框 + 16 padding + 24 内容）
pub const BASE_SIZE_HORIZONTAL: (f64, f64) = (231.0, 44.0);
/// 垂直（宽固定 48 = 边框 2 + padding 16 + 最宽控件 30（共享开关）；
///        高 = 边框 2 + padding 16 + 手柄 16 + gap 4 + 旋转 24(+mb 4) + gap 4 + 开关 18(+mb 6)
///             + gap 4 + 图标区(mb 6) + gap 4 + 功能组 111 = 219 + 图标区高）
pub const BASE_SIZE_VERTICAL: (f64, f64) = (48.0, 219.0);
/// ⠿ 拖动手柄中心距布局端部的偏移（物理像素，padding 8 + 手柄宽 16 / 2，两布局一致）
pub const SLIDER_AXIS_OFFSET: f64 = 16.0;
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
    /// 目标宽（物理像素）：前端 setSize 直接使用，避免前后端两处硬编码尺寸漂移
    pub w: f64,
    /// 目标高（物理像素）
    pub h: f64,
}

/// 应用图标区跨度：n 个图标 → n*20 + (n-1)*4（n=0 → 0）
/// 水平态为图标区宽度，垂直态为图标区高度（两者公式一致）
pub fn icons_span(count: usize) -> f64 {
    if count == 0 {
        0.0
    } else {
        count as f64 * ICON_SIZE + (count as f64 - 1.0) * ICON_GAP
    }
}

/// 布局方向 + 应用图标数 → 窗口尺寸（物理像素）
///
/// 【内容驱动】窗口尺寸随共享应用数自适应：应用图标区是唯一可变的部分，
/// 其余控件尺寸固定，故窗口尺寸 = 基础尺寸 + 图标区跨度。
/// 水平：宽 = 231 + icons_span(n)，高固定 44
/// 垂直：宽固定 48，高 = 219 + icons_span(n)
pub fn target_size(o: Orientation, icon_count: usize) -> (f64, f64) {
    match o {
        Orientation::Horizontal => (BASE_SIZE_HORIZONTAL.0 + icons_span(icon_count), BASE_SIZE_HORIZONTAL.1),
        Orientation::Vertical => (BASE_SIZE_VERTICAL.0, BASE_SIZE_VERTICAL.1 + icons_span(icon_count)),
    }
}

/// 解析布局方向："vertical"→Vertical，其余（含空串/非法）→Horizontal
pub fn parse_orientation(s: &str) -> Orientation {
    if s == "vertical" {
        Orientation::Vertical
    } else {
        Orientation::Horizontal
    }
}

/// 工作区居中坐标（绝对屏幕坐标）
///
/// 【多显示器】工作区原点可能非 0 或为负（副屏在主屏左侧时 work.x < 0），
/// 故居中结果必须加回 work.x/work.y 才是绝对坐标
pub fn default_center(work: Rect, w: f64, h: f64) -> (f64, f64) {
    (work.x + (work.w - w) / 2.0, work.y + (work.h - h) / 2.0)
}

/// 悬浮窗【默认位置】（物理像素，屏幕绝对坐标）：当前显示器工作区水平居中 + 顶端吸附
///
/// 【启动机制·FR-021】每次开启悬浮窗（托盘/主界面开关/启动自建）都恢复到本位置，
/// 不再使用落盘的 x/y。目的：悬浮窗被拖到屏幕外、分辨率变化或异常卡位后，
/// 用户随时可以通过"关掉再打开"把它找回来，不会出现"找不到悬浮窗"的僵局。
///
/// 公式：`x = work.x + (work.w - 窗口外层宽) / 2`，`y = work.y`
/// - work 为 GetMonitorInfoW rcWork；多显示器下工作区原点可能非 0，必须计入 work.x/work.y，
///   否则会落到主显示器上，与"所在屏幕居中"的语义不符
/// - 窗口宽取【外层】宽（含 Windows 隐形边框）：边框左右对称，内层与外层同时居中，
///   视觉上无偏移；调用方统一用 Tauri 的 outer_size() 获取
pub fn default_float_position(work: Rect, w: f64) -> (f64, f64) {
    (work.x + (work.w - w) / 2.0, work.y)
}

/// 边缘吸附：四边距离最小值 ≤ threshold 时吸附该边（顶/底=水平，左/右=垂直）；
/// 超过阈值原样返回并保持传入方向。距离并列优先级：顶 > 底 > 左 > 右。
///
/// 【内容驱动】窗口尺寸随应用图标数变化，故吸附/切换后的目标尺寸一律取
/// `target_size(目标方向, icon_count)`，不得用切换前的 rect.w/rect.h，
/// 否则贴右/贴底时短边窗口会悬空 |长边-短边|。
/// 【多显示器·关键】工作区原点可能非 0 或为负（副屏在主屏左侧时 work.x = -2880、
/// work.y = -512），因此所有边距一律换算到【相对工作区原点】的坐标后再比较，
/// 吸附结果再平移回绝对坐标。若直接拿绝对坐标与 work.w/work.h 比较，
/// 副屏上会算出负边距（如 d_l = -2870），从而误判为"贴左"并把窗口吸到主屏边缘。
pub fn snap_to_edge(rect: Rect, work: Rect, threshold: f64, current: Orientation, icon_count: usize) -> SnapResult {
    // 相对工作区原点的坐标（副屏场景下可为负）
    let rel_x = rect.x - work.x;
    let rel_y = rect.y - work.y;
    let d_t = rel_y;
    let d_b = work.h - (rel_y + rect.h);
    let d_l = rel_x;
    let d_r = work.w - (rel_x + rect.w);
    let min = d_t.min(d_b).min(d_l).min(d_r);
    if min > threshold {
        let (w, h) = target_size(current, icon_count);
        return SnapResult { x: rect.x, y: rect.y, orientation: current, w, h };
    }
    // 分支顺序即并列优先级：顶 > 底 > 左 > 右；贴底/贴右必须用目标布局尺寸
    // （如垂直窗贴底 → 水平窗矮，若用 rect.h 会悬空）
    let (hw, hh) = target_size(Orientation::Horizontal, icon_count);
    let (vw, vh) = target_size(Orientation::Vertical, icon_count);
    if d_t == min {
        SnapResult { x: rect.x, y: work.y, orientation: Orientation::Horizontal, w: hw, h: hh }
    } else if d_b == min {
        SnapResult { x: rect.x, y: work.y + work.h - hh, orientation: Orientation::Horizontal, w: hw, h: hh }
    } else if d_l == min {
        SnapResult { x: work.x, y: rect.y, orientation: Orientation::Vertical, w: vw, h: vh }
    } else {
        SnapResult { x: work.x + work.w - vw, y: rect.y, orientation: Orientation::Vertical, w: vw, h: vh }
    }
}

/// 越界修正：把窗口位置收进工作区（分辨率变化/拔显示器自愈）
///
/// 【多显示器】钳制区间必须加上工作区原点 work.x/work.y，
/// 否则副屏上的窗口会被强行拉回主屏坐标系（副屏在左侧时上限会算成负数）
pub fn clamp_to_work_area(x: f64, y: f64, w: f64, h: f64, work: Rect) -> (f64, f64) {
    let cx = if w >= work.w { work.x } else { x.clamp(work.x, work.x + work.w - w) };
    let cy = if h >= work.h { work.y } else { y.clamp(work.y, work.y + work.h - h) };
    (cx, cy)
}

/// 布局切换时的滑块锚点补偿：以 ⠿ 拖动滑块为旋转中心，切换后滑块中心保持不动
/// （水平→垂直：贴边轴 x 不变，补偿 y；垂直→水平：贴边轴 y 不变，补偿 x）
pub fn anchor_adjust(rect: Rect, mut r: SnapResult, current: Orientation, work: Rect, icon_count: usize) -> SnapResult {
    if r.orientation == current {
        let (w, h) = target_size(r.orientation, icon_count);
        return SnapResult { x: r.x, y: r.y, orientation: r.orientation, w, h };
    }
    if r.orientation == Orientation::Vertical {
        // 水平手柄中心 y = rect.y + rect.h/2（垂直居中）；垂直布局手柄中心 = r.y + SLIDER_AXIS_OFFSET
        // 【多显示器】上限加回 work.y，钳制区间为 [work.y, work.y + max_y]
        let max_y = (work.h - target_size(Orientation::Vertical, icon_count).1).max(0.0);
        r.y = (rect.y + rect.h / 2.0 - SLIDER_AXIS_OFFSET).clamp(work.y, work.y + max_y);
    } else {
        // 垂直手柄中心 x = rect.x + rect.w/2（水平居中）；水平布局手柄中心 = r.x + SLIDER_AXIS_OFFSET
        // 【多显示器】上限加回 work.x，钳制区间为 [work.x, work.x + max_x]
        let max_x = (work.w - target_size(Orientation::Horizontal, icon_count).0).max(0.0);
        r.x = (rect.x + rect.w / 2.0 - SLIDER_AXIS_OFFSET).clamp(work.x, work.x + max_x);
    }
    let (w, h) = target_size(r.orientation, icon_count);
    SnapResult { x: r.x, y: r.y, orientation: r.orientation, w, h }
}

/// 手动旋转布局（FR-016）：水平 ↔ 垂直
/// 旋转中心 = ⠿ 拖动手柄中心（与边缘吸附触发的布局切换同一锚点），切换后 clamp 进工作区
pub fn rotate_to(rect: Rect, current: Orientation, work: Rect, icon_count: usize) -> SnapResult {
    let target = match current {
        Orientation::Horizontal => Orientation::Vertical,
        Orientation::Vertical => Orientation::Horizontal,
    };
    let raw = SnapResult { x: rect.x, y: rect.y, orientation: target, w: 0.0, h: 0.0 };
    let adj = anchor_adjust(rect, raw, current, work, icon_count);
    let (cx, cy) = clamp_to_work_area(adj.x, adj.y, adj.w, adj.h, work);
    SnapResult { x: cx, y: cy, orientation: adj.orientation, w: adj.w, h: adj.h }
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
/// 已存在时无操作；【FR-021】位置一律取默认位置（水平居中 + 顶端吸附），不使用落盘坐标
pub fn create_float_window(app: &AppHandle, cfg: &FloatWindowConfig, visible: bool) -> tauri::Result<()> {
    if app.get_webview_window("float").is_some() {
        return Ok(());
    }
    let vertical = parse_orientation(&cfg.orientation) == Orientation::Vertical;
    // 初始尺寸取【基础尺寸】（图标区为空）：前端挂载后按实际共享应用数调整为 target_size
    let (w, h) = if vertical { BASE_SIZE_VERTICAL } else { BASE_SIZE_HORIZONTAL };
    // builder 的 inner_size/position 为逻辑像素签名，按主窗口 scale 换算（目标尺寸为物理像素）
    let scale = app
        .get_webview_window("main")
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or(1.0);
    // 【FR-021】每次开启恢复到默认位置（水平居中 + 顶端吸附），
    // 不再读取 cfg.x/cfg.y，避免异常位置（如拖出屏幕）后无法找回
    let (px, py) = match work_area_of_float_window(app) {
        Some(work) => default_float_position(work, w),
        None => (0.0, 0.0),
    };
    let builder = WebviewWindowBuilder::new(app, "float", WebviewUrl::App("index.html".into()))
        .title("AudioShare 悬浮窗")
        .decorations(false)
        .always_on_top(true)
        .transparent(true)
        // Windows 上 WebView2 透明窗口默认白色合成底，显式透明背景色避免白底矩形（TC-046）
        .background_color(Color(0, 0, 0, 0))
        .skip_taskbar(true)
        .resizable(false)
        .inner_size(w / scale, h / scale)
        .position(px / scale, py / scale)
        .visible(visible);
    builder.build()?;
    // 【FR-021·Windows 实测】builder 的 position() 在 Windows 上不生效：
    // 窗口会被创建在鼠标光标处（如 (1872,195)），而非指定坐标。
    // 故建窗后统一显式 set_position 到默认位置，保证"开启即在屏幕水平居中 + 顶端"。
    if let Some(win) = app.get_webview_window("float") {
        // 用【外层】宽居中（含隐形边框，内外层同时居中无视觉偏移）
        let outer_w = win.outer_size().map(|s| s.width as f64).unwrap_or(w);
        let (fx, fy) = match work_area_of_float_window(app) {
            Some(work) => default_float_position(work, outer_w),
            None => (px, py),
        };
        let _ = win.set_position(PhysicalPosition::new(fx as i32, fy as i32));
        log::info!(target: "float", "悬浮窗已定位到默认位置 ({fx},{fy}) 外层宽={outer_w}");
    }
    log::info!(target: "float", "悬浮窗已创建 visible={visible} 位置=({px},{py})");
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

/// 显示悬浮窗：不存在则按配置创建，存在则 show（托盘/设置开关共用）。
/// 显示即启用：落盘 enabled=true 并通知前端（主界面开关同步，托盘入口同样生效）
///
/// 【线程约束】若在主线程调用（托盘菜单回调即在主线程），窗口创建会死锁，
/// 故派发到工作线程执行；调用方为异步 command 时本就在工作线程，直接执行。
pub fn show_float_window(app: &AppHandle) {
    if is_main_thread() {
        let app2 = app.clone();
        std::thread::spawn(move || show_float_window(&app2));
        return;
    }
    if app.get_webview_window("float").is_none() {
        let cfg = load_float_config(app);
        if let Err(e) = create_float_window(app, &cfg, true) {
            log::error!(target: "float", "创建悬浮窗失败: {e}");
            return;
        }
    }
    if let Some(w) = app.get_webview_window("float") {
        let _ = w.show();
        // 不抢焦点：悬浮窗是常驻工具窗，set_focus 会把键盘焦点夺走，
        // 鼠标恰好悬停在按钮上时会误触（曾导致共享被意外开启、闭麦被反复翻转）
        // 重显后补回置顶：Windows 上 hide→show 可能丢失 WS_EX_TOPMOST 标志，
        // 否则会被全屏应用盖住（用户看到悬浮窗消失）
        let _ = w.set_always_on_top(true);
        // 【时序关键】先广播"即将重显"，让前端进入位置噪声抑制窗口，
        // 再定位窗口：set_position 触发的 onMoved 会在广播后到达，否则会被当成用户拖动而误吸附
        // （默认位置 y=0 属"贴顶"，误吸附会把用户的垂直布局改成水平）
        let _ = app.emit("float_window_shown", ());
        // 【FR-021】每次开启恢复到默认位置（水平居中 + 顶端吸附）：
        // 悬浮窗被拖出屏幕/异常卡位后，"关掉再打开"即可找回，不会丢失入口
        if let Some(work) = work_area_of_float_window(&app) {
            let cur_w = w.outer_size().map(|s| s.width as f64).unwrap_or(BASE_SIZE_HORIZONTAL.0);
            let (px, py) = default_float_position(work, cur_w);
            let _ = w.set_position(PhysicalPosition::new(px as i32, py as i32));
            log::info!(target: "float", "悬浮窗已恢复到默认位置 ({px},{py})");
        }
    }
    let mut cfg = load_float_config(app);
    if !cfg.enabled {
        cfg.enabled = true;
        save_float_config(app, &cfg);
    }
    let _ = app.emit("float_enabled_changed", true);
    log::info!(target: "float", "悬浮窗已显示");
}

// ---------- 命令（前端 invoke） ----------

/// 读取悬浮窗配置（前端初始化）
#[tauri::command]
pub fn get_float_config(app: AppHandle) -> FloatWindowConfig {
    load_float_config(&app)
}

/// 开关悬浮窗：on=true 显示悬浮窗并自动隐藏主界面（第3期验收反馈），
/// on=false 隐藏悬浮窗；enabled 落盘与前端事件由 show/hide_float_window 统一处理
///
/// 【线程约束·关键】必须为 async command：Tauri v2 的同步 command 在主线程执行，
/// 而在主线程调用 `WebviewWindowBuilder::build()` 会自等待死锁（见 MAIN_THREAD_ID 注释）。
/// async command 在 async runtime 线程执行，build() 投递到主线程 event loop 后
/// 由 event loop 正常处理创建，不会卡死。
#[tauri::command]
pub async fn set_float_enabled(app: AppHandle, on: bool) -> Result<(), String> {
    log::info!(target: "float", "set_float_enabled on={on} 线程={:?}", std::thread::current().id());
    if on {
        show_float_window(&app);
        if let Some(m) = app.get_webview_window("main") {
            let _ = m.hide();
        }
    } else {
        hide_float_window(&app);
        // 关闭悬浮窗 → 主窗口回归：开启悬浮窗时主窗口被隐藏，
        // 若关闭后不恢复，用户将既无悬浮窗也无主界面（只能从托盘唤回）
        if let Some(m) = app.get_webview_window("main") {
            let _ = m.show();
            let _ = m.unminimize();
            let _ = m.set_focus();
            // Windows 从后台恢复窗口时 SetForegroundWindow 常被系统拦截，
            // 短暂置顶再取消，确保用户真的能看到主界面
            let _ = m.set_always_on_top(true);
            let m2 = m.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1200));
                let _ = m2.set_always_on_top(false);
            });
        }
    }
    Ok(())
}

/// 边缘吸附（前端 onMoved 防抖 200ms 后调用）：当前方向按窗口尺寸推断（w>h=水平）；
/// 布局切换时以 ⠿ 滑块为锚点补偿另一轴（消除窗口跳位）
///
/// icon_count = 当前共享应用图标数（上限 3），决定目标布局下的窗口尺寸
#[tauri::command]
pub fn snap_float(app: AppHandle, x: f64, y: f64, w: f64, h: f64, icon_count: usize) -> Result<SnapResult, String> {
    let work = work_area_of_float_window(&app).ok_or("无法获取显示器工作区")?;
    let rect = Rect { x, y, w, h };
    let current = if w > h { Orientation::Horizontal } else { Orientation::Vertical };
    let r = snap_to_edge(rect, work, SNAP_THRESHOLD, current, icon_count);
    Ok(anchor_adjust(rect, r, current, work, icon_count))
}

/// 手动旋转布局（FR-016）：水平 ↔ 垂直
/// 返回目标坐标与尺寸（物理像素），前端据此做 6 步插值过渡并落盘
#[tauri::command]
pub fn rotate_float(app: AppHandle, x: f64, y: f64, w: f64, h: f64, icon_count: usize) -> Result<SnapResult, String> {
    let work = work_area_of_float_window(&app).ok_or("无法获取显示器工作区")?;
    let rect = Rect { x, y, w, h };
    let current = if w > h { Orientation::Horizontal } else { Orientation::Vertical };
    Ok(rotate_to(rect, current, work, icon_count))
}

/// 目标布局方向 + 应用图标数 → 窗口尺寸（物理像素）
/// 前端在共享应用数变化时调用，按结果调整窗口尺寸（尺寸口径单一真源在后端）
#[tauri::command]
pub fn float_target_size(orientation: String, icon_count: usize) -> (f64, f64) {
    target_size(parse_orientation(&orientation), icon_count)
}

/// 尺寸调整后把窗口收进工作区（返回修正后的 x/y）
/// 应用图标数变化 → 窗口变宽/变高 → 可能超出工作区，需拉回
#[tauri::command]
pub fn fit_float_position(app: AppHandle, x: f64, y: f64, w: f64, h: f64) -> Result<(f64, f64), String> {
    let work = work_area_of_float_window(&app).ok_or("无法获取显示器工作区")?;
    Ok(clamp_to_work_area(x, y, w, h, work))
}

/// 当前【默认位置】（FR-021）：按悬浮窗当前【外层】宽度计算的水平居中 + 顶端吸附坐标
///
/// 前端用途：共享应用数变化导致宽度变化时，居中 x 需重算；
/// 前端挂载时也可能早于后端建窗定位，故由本命令提供唯一真源做一次对齐。
#[tauri::command]
pub fn float_default_position(app: AppHandle) -> (f64, f64) {
    let outer_w = app
        .get_webview_window("float")
        .and_then(|w| w.outer_size().ok())
        .map(|s| s.width as f64)
        .unwrap_or(BASE_SIZE_HORIZONTAL.0);
    match work_area_of_float_window(&app) {
        Some(work) => default_float_position(work, outer_w),
        None => (0.0, 0.0),
    }
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

    /// 共享应用图标数（上限 3，与前端 MAX_VISIBLE_ICONS 一致）
    const ICONS: usize = 3;

    /// TC-047：距顶 ≤ 阈值 → 贴顶，水平布局
    #[test]
    fn snap_to_top_edge_is_horizontal() {
        let r = snap_to_edge(Rect { x: 600.0, y: 10.0, w: 299.0, h: 44.0 }, work(), SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        assert_eq!(r.x, 600.0);
        assert_eq!(r.y, 0.0);
        assert_eq!(r.orientation, Orientation::Horizontal);
    }

    /// TC-047：距底 ≤ 阈值 → 贴底，水平布局
    #[test]
    fn snap_to_bottom_edge_is_horizontal() {
        let work = work();
        let r = snap_to_edge(Rect { x: 600.0, y: work.h - 44.0 - 15.0, w: 299.0, h: 44.0 }, work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        assert_eq!(r.y, work.h - 44.0);
        assert_eq!(r.orientation, Orientation::Horizontal);
    }

    /// TC-047/048：距左 ≤ 阈值 → 贴左，垂直布局
    #[test]
    fn snap_to_left_edge_is_vertical() {
        let r = snap_to_edge(Rect { x: 12.0, y: 400.0, w: 299.0, h: 44.0 }, work(), SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        assert_eq!(r.x, 0.0);
        assert_eq!(r.y, 400.0);
        assert_eq!(r.orientation, Orientation::Vertical);
    }

    /// TC-047/048：距右 ≤ 阈值 → 贴右，垂直布局（用目标布局宽 48 而非当前宽 299）
    #[test]
    fn snap_to_right_edge_is_vertical() {
        let work = work();
        let r = snap_to_edge(Rect { x: work.w - 299.0 - 8.0, y: 400.0, w: 299.0, h: 44.0 }, work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        assert_eq!(r.x, work.w - target_size(Orientation::Vertical, ICONS).0);
        assert_eq!(r.orientation, Orientation::Vertical);
    }

    /// 验收反馈：垂直窗拖到右边缘 → 贴右且右边缘贴齐（切换后无悬空/错位）
    #[test]
    fn snap_to_bottom_from_vertical_uses_target_height() {
        let work = work();
        // 垂直窗（48×287）底部距屏幕底 8px → 贴底 → 水平窗（299×44）底部必须贴齐
        let r = snap_to_edge(Rect { x: 600.0, y: work.h - 287.0 - 8.0, w: 48.0, h: 287.0 }, work, SNAP_THRESHOLD, Orientation::Vertical, ICONS);
        assert_eq!(r.orientation, Orientation::Horizontal);
        assert_eq!(r.y, work.h - target_size(Orientation::Horizontal, ICONS).1); // 996 = 1040 - 44
    }

    /// 阈值外不吸附：位置不变、方向保持传入值（含从垂直拖回中间）
    #[test]
    fn no_snap_when_far_from_edges() {
        let rect = Rect { x: 500.0, y: 500.0, w: 299.0, h: 44.0 };
        let r = snap_to_edge(rect, work(), SNAP_THRESHOLD, Orientation::Vertical, ICONS);
        assert_eq!((r.x, r.y), (500.0, 500.0));
        assert_eq!(r.orientation, Orientation::Vertical); // 自由位置方向由调用方决定
    }

    /// 恰等于阈值 → 吸附（≤ 判定）
    #[test]
    fn snap_at_exact_threshold() {
        let r = snap_to_edge(Rect { x: SNAP_THRESHOLD, y: 400.0, w: 299.0, h: 44.0 }, work(), SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        assert_eq!(r.x, 0.0);
        assert_eq!(r.orientation, Orientation::Vertical);
    }

    /// 多边并列优先级：顶 > 底 > 左 > 右（角部并列场景逐一验证）
    #[test]
    fn tie_break_priority_top_bottom_left_right() {
        let work = work();
        // 四边全贴（左上角 0,0）：dT=dL=0 并列 → 顶优先：水平贴顶
        let r = snap_to_edge(Rect { x: 0.0, y: 0.0, w: 299.0, h: 44.0 }, work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        assert_eq!(r.orientation, Orientation::Horizontal);
        assert_eq!((r.x, r.y), (0.0, 0.0));
        // dT 与 dR 并列（右上角 8px）→ 顶优先：水平贴顶
        let r2 = snap_to_edge(Rect { x: work.w - 299.0 - 8.0, y: 8.0, w: 299.0, h: 44.0 }, work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        assert_eq!(r2.orientation, Orientation::Horizontal);
        assert_eq!(r2.y, 0.0);
        // dB 与 dL 并列（左下角 8px）→ 底优先（底在左前）：水平贴底
        let r3 = snap_to_edge(Rect { x: 8.0, y: work.h - 44.0 - 8.0, w: 299.0, h: 44.0 }, work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        assert_eq!(r3.orientation, Orientation::Horizontal);
        assert_eq!((r3.x, r3.y), (8.0, work.h - 44.0));
        // dB 与 dR 并列（右下角 6px）→ 底优先：水平贴底
        let r4 = snap_to_edge(Rect { x: work.w - 299.0 - 6.0, y: work.h - 44.0 - 6.0, w: 299.0, h: 44.0 }, work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
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

    /// FR-021：默认位置 = 所在屏幕水平居中 + 顶端吸附（距顶 0px）
    /// 开启悬浮窗时无条件回到该位置，避免拖出屏幕后无法找回
    /// 注：w 为窗口【外层】宽（含隐形边框），由调用方用 outer_size() 获取
    #[test]
    fn default_float_position_is_top_center() {
        let work = work();
        // 外层宽 247（内层 231 + 边框 16）→ x = (1920-247)/2 = 836.5，y = 0
        assert_eq!(default_float_position(work, 247.0), (836.5, 0.0));
        // 内层基础宽 231 → x = (1920-231)/2 = 844.5，y = 0
        assert_eq!(default_float_position(work, 231.0), (844.5, 0.0));
        // 3 个应用（内层 299 宽）→ x = (1920-299)/2 = 810.5
        assert_eq!(default_float_position(work, 299.0), (810.5, 0.0));
        // 垂直态内层 48 宽 → x = (1920-48)/2 = 936，y 仍为 0（顶端吸附）
        assert_eq!(default_float_position(work, 48.0), (936.0, 0.0));
    }

    /// FR-021：默认位置随工作区变化（分辨率/多显示器自愈）
    #[test]
    fn default_float_position_follows_work_area() {
        let small = Rect { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 };
        assert_eq!(default_float_position(small, 231.0), ((1280.0 - 231.0) / 2.0, 0.0));
    }

    /// FR-021：多显示器下工作区原点非 0（如副屏在主屏右侧）→ 默认位置须落在该屏内
    #[test]
    fn default_float_position_respects_work_area_origin() {
        // 副屏工作区：x=1920, y=0, 宽 2560
        let second = Rect { x: 1920.0, y: 0.0, w: 2560.0, h: 1440.0 };
        let (x, y) = default_float_position(second, 231.0);
        assert_eq!(x, 1920.0 + (2560.0 - 231.0) / 2.0);
        assert_eq!(y, 0.0);
        // 位于该屏内（不跑到主屏）
        assert!(x >= second.x && x + 231.0 <= second.x + second.w);
    }

    /// ---------- FR-022：多显示器（双屏）适配 ----------

    /// 副屏工作区（副屏在主屏【左侧】，故原点为负）：x=-2880, y=-512, 1646x981
    fn secondary_work() -> Rect {
        Rect { x: -2880.0, y: -512.0, w: 1646.0, h: 981.0 }
    }

    /// FR-022：拖到副屏左边缘 → 吸附到【副屏】左边缘（x = work.x），不得跳回主屏（x = 0）
    #[test]
    fn snap_left_on_secondary_screen_stays_on_it() {
        let work = secondary_work();
        let r = snap_to_edge(
            Rect { x: work.x + 10.0, y: -200.0, w: 299.0, h: 44.0 },
            work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS,
        );
        assert_eq!(r.orientation, Orientation::Vertical);
        assert_eq!(r.x, work.x); // -2880：副屏左边缘
        assert_eq!(r.y, -200.0);
    }

    /// FR-022：拖到副屏顶边缘 → 吸附到副屏顶端（y = work.y），不得跳到主屏顶端（y = 0）
    #[test]
    fn snap_top_on_secondary_screen_stays_on_it() {
        let work = secondary_work();
        let r = snap_to_edge(
            Rect { x: -2000.0, y: work.y + 5.0, w: 299.0, h: 44.0 },
            work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS,
        );
        assert_eq!(r.orientation, Orientation::Horizontal);
        assert_eq!(r.y, work.y); // -512：副屏顶端
        assert_eq!(r.x, -2000.0);
    }

    /// FR-022：拖到副屏右边缘 → 吸附到副屏右边缘（x = work.x + work.w - 目标垂直宽）
    #[test]
    fn snap_right_on_secondary_screen_stays_on_it() {
        let work = secondary_work();
        let r = snap_to_edge(
            Rect { x: work.x + work.w - 299.0 - 8.0, y: -100.0, w: 299.0, h: 44.0 },
            work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS,
        );
        assert_eq!(r.orientation, Orientation::Vertical);
        assert_eq!(r.x, work.x + work.w - target_size(Orientation::Vertical, ICONS).0);
    }

    /// FR-022：拖到副屏底边缘 → 吸附到副屏底端（y = work.y + work.h - 目标水平高）
    #[test]
    fn snap_bottom_on_secondary_screen_stays_on_it() {
        let work = secondary_work();
        let r = snap_to_edge(
            Rect { x: -2000.0, y: work.y + work.h - 44.0 - 8.0, w: 299.0, h: 44.0 },
            work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS,
        );
        assert_eq!(r.orientation, Orientation::Horizontal);
        assert_eq!(r.y, work.y + work.h - target_size(Orientation::Horizontal, ICONS).1);
    }

    /// FR-022：副屏上远离边缘 → 不吸附，且位置保持绝对坐标不变
    #[test]
    fn no_snap_inside_secondary_screen_keeps_absolute_pos() {
        let work = secondary_work();
        let rect = Rect { x: -2000.0, y: -100.0, w: 299.0, h: 44.0 };
        let r = snap_to_edge(rect, work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        assert_eq!((r.x, r.y), (-2000.0, -100.0));
    }

    /// FR-022：越界修正必须钳制在【副屏】工作区内，不得拉回主屏坐标系
    #[test]
    fn clamp_stays_in_secondary_work_area() {
        let work = secondary_work();
        // 完全越界（左上）→ 副屏左上角
        assert_eq!(clamp_to_work_area(-4000.0, -900.0, 299.0, 44.0, work), (work.x, work.y));
        // 右下越界 → 副屏右下角（y=500 超出下边界 425）
        assert_eq!(
            clamp_to_work_area(0.0, 500.0, 299.0, 44.0, work),
            (work.x + work.w - 299.0, work.y + work.h - 44.0)
        );
        // 区内不动
        assert_eq!(clamp_to_work_area(-2000.0, -100.0, 299.0, 44.0, work), (-2000.0, -100.0));
    }

    /// FR-022：副屏上的锚点补偿结果仍落在副屏工作区内（不跳主屏）
    #[test]
    fn anchor_adjust_on_secondary_screen_stays_in_it() {
        let work = secondary_work();
        let rect = Rect { x: work.x + 8.0, y: -100.0, w: 299.0, h: 44.0 };
        let r = snap_to_edge(rect, work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        let r = anchor_adjust(rect, r, Orientation::Horizontal, work, ICONS);
        assert_eq!(r.orientation, Orientation::Vertical);
        assert_eq!(r.x, work.x);
        // 滑块中心锚点：y = rect.y + rect.h/2 - SLIDER_AXIS_OFFSET
        assert_eq!(r.y, -100.0 + 22.0 - SLIDER_AXIS_OFFSET);
        assert!(r.y >= work.y && r.y + r.h <= work.y + work.h);
    }

    /// FR-022：副屏工作区居中 → 绝对坐标落在副屏内
    #[test]
    fn default_center_on_secondary_screen() {
        let work = secondary_work();
        let (x, y) = default_center(work, 299.0, 44.0);
        assert_eq!(x, work.x + (work.w - 299.0) / 2.0);
        assert_eq!(y, work.y + (work.h - 44.0) / 2.0);
        assert!(x >= work.x && x + 299.0 <= work.x + work.w);
    }

    /// 验收反馈：水平→垂直以 ⠿ 滑块为锚点，y 补偿使滑块中心不动（原滑块中心 y=422 → 新 y=402）
    #[test]
    fn anchor_adjust_h_to_v_keeps_slider_center() {
        let rect = Rect { x: 12.0, y: 400.0, w: 299.0, h: 44.0 };
        let r = snap_to_edge(rect, work(), SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        let r = anchor_adjust(rect, r, Orientation::Horizontal, work(), ICONS);
        assert_eq!(r.orientation, Orientation::Vertical);
        assert_eq!(r.x, 0.0); // 贴边轴不变
        assert_eq!(r.y, 400.0 + 22.0 - SLIDER_AXIS_OFFSET); // 402.0
    }

    /// 验收反馈：垂直→水平以 ⠿ 滑块为锚点，x 补偿使滑块中心不动（垂直贴左拖到顶，滑块中心 x=522 → 新 x=502）
    #[test]
    fn anchor_adjust_v_to_h_keeps_slider_center() {
        let rect = Rect { x: 500.0, y: 5.0, w: 48.0, h: 287.0 };
        let r = snap_to_edge(rect, work(), SNAP_THRESHOLD, Orientation::Vertical, ICONS);
        let r = anchor_adjust(rect, r, Orientation::Vertical, work(), ICONS);
        assert_eq!(r.orientation, Orientation::Horizontal);
        assert_eq!(r.y, 0.0); // 贴边轴不变
        assert_eq!(r.x, 500.0 + 48.0 / 2.0 - SLIDER_AXIS_OFFSET); // 508.0
    }

    /// 验收反馈：锚点补偿后越界 → 收进工作区（靠近底部切换垂直布局）
    #[test]
    fn anchor_adjust_clamps_into_work_area() {
        let work = work();
        // 距左 8px（距底 96px 超阈值）→ 左侧吸附 → 垂直；滑块中心 y=922 越界 → clamp 到底
        let rect2 = Rect { x: 8.0, y: 900.0, w: 299.0, h: 44.0 };
        let r2 = snap_to_edge(rect2, work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        assert_eq!(r2.orientation, Orientation::Vertical);
        let r2 = anchor_adjust(rect2, r2, Orientation::Horizontal, work, ICONS);
        assert_eq!(r2.x, 0.0);
        assert_eq!(r2.y, work.h - target_size(Orientation::Vertical, ICONS).1); // clamp 到底部
    }

    /// 同方向不补偿：位置原样
    #[test]
    fn anchor_adjust_same_orientation_noop() {
        let rect = Rect { x: 600.0, y: 10.0, w: 299.0, h: 44.0 };
        let r = snap_to_edge(rect, work(), SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        let r2 = anchor_adjust(rect, r, Orientation::Horizontal, work(), ICONS);
        assert_eq!((r2.x, r2.y), (r.x, r.y));
    }

    /// 验收反馈：贴右切换（水平→垂直）右边缘贴齐 + y 按滑块锚点补偿，无错位
    #[test]
    fn anchor_adjust_snap_right_no_misalignment() {
        let work = work();
        let rect = Rect { x: work.w - 299.0 - 8.0, y: 400.0, w: 299.0, h: 44.0 };
        let r = snap_to_edge(rect, work, SNAP_THRESHOLD, Orientation::Horizontal, ICONS);
        let r = anchor_adjust(rect, r, Orientation::Horizontal, work, ICONS);
        assert_eq!(r.x, work.w - target_size(Orientation::Vertical, ICONS).0); // 贴右：右边缘 work.w
        assert_eq!(r.y, 400.0 + 22.0 - SLIDER_AXIS_OFFSET); // 滑块 y 中心不动
    }

    /// 验收反馈：贴底切换（垂直→水平）底边缘贴齐 + x 按滑块锚点补偿，无错位
    #[test]
    fn anchor_adjust_snap_bottom_no_misalignment() {
        let work = work();
        let rect = Rect { x: 600.0, y: work.h - 287.0 - 8.0, w: 48.0, h: 287.0 };
        let r = snap_to_edge(rect, work, SNAP_THRESHOLD, Orientation::Vertical, ICONS);
        let r = anchor_adjust(rect, r, Orientation::Vertical, work, ICONS);
        assert_eq!(r.y, work.h - target_size(Orientation::Horizontal, ICONS).1); // 贴底：底边缘 work.h
        assert_eq!(r.x, 600.0 + 48.0 / 2.0 - SLIDER_AXIS_OFFSET); // 滑块 x 中心不动
    }

    /// 方向解析：vertical→垂直；horizontal/空串/非法→水平
    #[test]
    fn parse_orientation_valid_and_fallback() {
        assert_eq!(parse_orientation("vertical"), Orientation::Vertical);
        assert_eq!(parse_orientation("horizontal"), Orientation::Horizontal);
        assert_eq!(parse_orientation(""), Orientation::Horizontal);
        assert_eq!(parse_orientation("diagonal"), Orientation::Horizontal);
    }

    /// 图标区跨度：0 个 → 0；1 个 → 20；2 个 → 44；3 个 → 68（n*20 + (n-1)*4）
    #[test]
    fn icons_span_follows_icon_count() {
        assert_eq!(icons_span(0), 0.0);
        assert_eq!(icons_span(1), 20.0);
        assert_eq!(icons_span(2), 44.0);
        assert_eq!(icons_span(3), 68.0);
    }

    /// 窗口尺寸随应用图标数自适应（内容驱动）：
    /// 水平 宽 = 231 + icons_span(n)，高恒 44；垂直 宽恒 48，高 = 219 + icons_span(n)
    #[test]
    fn target_size_grows_with_icon_count() {
        for n in 0..=3usize {
            let (hw, hh) = target_size(Orientation::Horizontal, n);
            assert_eq!(hw, 231.0 + icons_span(n));
            assert_eq!(hh, 44.0);
            let (vw, vh) = target_size(Orientation::Vertical, n);
            assert_eq!(vw, 48.0);
            assert_eq!(vh, 219.0 + icons_span(n));
        }
        // 3 个应用（上限）：水平 299×44，垂直 48×287
        assert_eq!(target_size(Orientation::Horizontal, 3), (299.0, 44.0));
        assert_eq!(target_size(Orientation::Vertical, 3), (48.0, 287.0));
        // 无应用：水平 231×44，垂直 48×219
        assert_eq!(target_size(Orientation::Horizontal, 0), (231.0, 44.0));
        assert_eq!(target_size(Orientation::Vertical, 0), (48.0, 219.0));
    }

    // ---------- T5：手动旋转（FR-016） ----------

    /// 水平 → 垂直：⠿ 手柄中心保持不动（水平 y 中心 = 400+22=422 → 垂直 r.y+16=422）
    #[test]
    fn rotate_to_horizontal_to_vertical_keeps_grip_center() {
        let rect = Rect { x: 600.0, y: 400.0, w: 299.0, h: 44.0 };
        let r = rotate_to(rect, Orientation::Horizontal, work(), ICONS);
        assert_eq!(r.orientation, Orientation::Vertical);
        assert_eq!(r.w, target_size(Orientation::Vertical, ICONS).0);
        assert_eq!(r.h, target_size(Orientation::Vertical, ICONS).1);
        assert_eq!(r.y, 400.0 + 22.0 - SLIDER_AXIS_OFFSET);
        // 手柄中心不变量：422
        assert_eq!(r.y + SLIDER_AXIS_OFFSET, 422.0);
        assert_eq!(r.x, 600.0);
    }

    /// 垂直 → 水平：⠿ 手柄中心保持不动（垂直 x 中心 = 500+24=524 → 水平 r.x+16=524）
    #[test]
    fn rotate_to_vertical_to_horizontal_keeps_grip_center() {
        let rect = Rect { x: 500.0, y: 100.0, w: 48.0, h: 287.0 };
        let r = rotate_to(rect, Orientation::Vertical, work(), ICONS);
        assert_eq!(r.orientation, Orientation::Horizontal);
        assert_eq!(r.w, target_size(Orientation::Horizontal, ICONS).0);
        assert_eq!(r.h, target_size(Orientation::Horizontal, ICONS).1);
        assert_eq!(r.x, 500.0 + 48.0 / 2.0 - SLIDER_AXIS_OFFSET);
        assert_eq!(r.x + SLIDER_AXIS_OFFSET, 524.0);
        assert_eq!(r.y, 100.0);
    }

    /// 旋转后越界 → clamp 进工作区（转水平后右边缘对齐；底部越界则拉回）
    /// 注：锚点补偿只作用于“变化轴”（V→H 补偿 x，y 保持不变），故 y 不自动贴底
    #[test]
    fn rotate_to_clamps_into_work_area() {
        let work = work();
        // 垂直窗位于右下角 → 转水平后右边缘贴齐，顶边位置保持
        let rect = Rect { x: work.w - 48.0 - 20.0, y: work.h - 287.0 - 20.0, w: 48.0, h: 287.0 };
        let r = rotate_to(rect, Orientation::Vertical, work, ICONS);
        assert_eq!(r.orientation, Orientation::Horizontal);
        assert_eq!(r.x, work.w - target_size(Orientation::Horizontal, ICONS).0);
        assert_eq!(r.y, work.h - 287.0 - 20.0);
        // 旋转后底部越界 → 拉回工作区
        let rect2 = Rect { x: 100.0, y: 1000.0, w: 48.0, h: 287.0 };
        let r2 = rotate_to(rect2, Orientation::Vertical, work, ICONS);
        assert_eq!(r2.y, work.h - target_size(Orientation::Horizontal, ICONS).1);
    }

    /// 旋转目标尺寸始终与方向匹配（前端直接用 r.w/r.h 做 setSize）
    #[test]
    fn rotate_result_size_matches_orientation() {
        let rect = Rect { x: 600.0, y: 400.0, w: 299.0, h: 44.0 };
        let r = rotate_to(rect, Orientation::Horizontal, work(), ICONS);
        assert_eq!((r.w, r.h), target_size(r.orientation, ICONS));
        let r2 = rotate_to(rect, Orientation::Vertical, work(), ICONS);
        assert_eq!((r2.w, r2.h), target_size(r2.orientation, ICONS));
    }

    /// 【内容驱动】旋转后的目标尺寸随应用图标数变化（0 个应用 → 垂直 48×219）
    #[test]
    fn rotate_target_size_follows_icon_count() {
        let rect = Rect { x: 600.0, y: 400.0, w: 231.0, h: 44.0 };
        let r = rotate_to(rect, Orientation::Horizontal, work(), 0);
        assert_eq!((r.w, r.h), (48.0, 219.0));
        let r3 = rotate_to(rect, Orientation::Horizontal, work(), 3);
        assert_eq!((r3.w, r3.h), (48.0, 287.0));
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
