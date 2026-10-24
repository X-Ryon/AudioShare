//! Tauri 命令层与音频引擎编排（技术设计§4 commands.rs）
//! 追溯：TC-001~TC-017
//! 混音线程每 tick 短暂持锁拉取各源帧（rtrb 消费需要 &mut），命令层临界区极短，无阻塞风险。
//! 阻塞式初始化（MicCapture/ProcessCapture）一律在锁外完成，避免持锁阻塞导致 UI 卡死。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_store::StoreExt;

use crate::audio::capture::ProcessCapture;
use crate::audio::device_enum::{self, AudioApp, MicDevice};
use crate::audio::mic_capture::MicCapture;
use crate::audio::mixer::{finalize_output, mix, rms_level, SourceTable};
use crate::audio::virtual_dev::VirtualSink;
use crate::hotkeys::{self, HotkeyConfig};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

// 图标缓存：exe 路径 -> base64 PNG
static ICON_CACHE: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Serialize)]
pub struct AppStatus {
    pub cable_installed: bool,
    pub master_on: bool,
    pub onboarding_done: bool,
    pub mic_id: String,
    pub shared: Vec<SharedInfo>,
    /// 输出闭麦状态（第3期 TC-053：不持久化，重启复位）
    pub output_muted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SharedInfo {
    pub pid: u32,
    pub exe: String,
    pub volume: u8,
}

struct SourceState {
    exe: String,
    capture: ProcessCapture,
    consumer: rtrb::Consumer<f32>,
    level: f32,
    /// 是否参与混音输出（false 时仍监控电平，但不混入虚拟声卡）
    shared: bool,
    /// 连续高电平 tick 数（播放判定，阈值 PLAY_THRESHOLD）
    playing_ticks: u32,
    /// 连续静音 tick 数（停止判定，需 SILENT_TICKS_STOP 个周期）
    silent_ticks: u32,
    /// 是否为自动共享（用户手动取消后不再自动重开）
    auto_shared: bool,
}

struct Engine {
    sources: HashMap<u32, SourceState>,
    gains: SourceTable,
    volumes: HashMap<u32, u8>,
    mic: Option<MicCapture>,
    mic_consumer: Option<rtrb::Consumer<f32>>,
    mic_id: String,
    master_on: bool,
    onboarding_done: bool,
    remember: bool,
    sink_running: bool,
    thread_stop: Arc<Mutex<bool>>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// 收藏的自动共享应用（小写 exe）
    favorites: HashSet<String>,
    /// 本次会话用户手动取消过自动共享的 exe（防止电平仍在而反复自动重开）
    auto_opt_out: HashSet<String>,
    /// 输出闭麦（第3期）：mix 后整帧清零；不持久化、不影响共享状态
    output_muted: bool,
}

impl Engine {
    fn new() -> Self {
        Self {
            sources: HashMap::new(),
            gains: SourceTable::new(),
            volumes: HashMap::new(),
            mic: None,
            mic_consumer: None,
            mic_id: String::new(),
            master_on: false,
            onboarding_done: false,
            remember: false,
            sink_running: false,
            thread_stop: Arc::new(Mutex::new(false)),
            thread: None,
            favorites: HashSet::new(),
            auto_opt_out: HashSet::new(),
            output_muted: false,
        }
    }

    fn stop_thread(&mut self) {
        *self.thread_stop.lock().unwrap() = true;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.sink_running = false;
    }
}

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

// ---------- 设置持久化（tauri-plugin-store）：onboarding 状态落盘 ----------
const SETTINGS_FILE: &str = "settings.json";

/// settings.json 绝对路径：统一存于 %APPDATA%\AudioShare（传相对路径会落到 identifier 目录）
fn settings_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    crate::data_dir(app).map(|d| d.join(SETTINGS_FILE))
}

fn load_onboarding_done(app: &AppHandle) -> bool {
    settings_path(app)
        .and_then(|p| app.store(p).ok())
        .and_then(|s| s.get("onboarding_done"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn save_onboarding_done(app: &AppHandle) {
    let Some(p) = settings_path(app) else { return };
    if let Ok(store) = app.store(p) {
        store.set("onboarding_done", json!(true));
        let _ = store.save();
    }
}

// ---------- 记住选择持久化：独立配置文件 remember.json，每次退出时覆盖写入 ----------
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RememberConfig {
    pub enabled: bool,
    /// 共享总开关状态（仅 enabled 时写回/恢复）
    #[serde(default)]
    pub master_on: bool,
    /// 记忆的麦克风设备 id（仅 enabled 时写回/恢复；空串=无记忆）
    #[serde(default)]
    pub mic: String,
}

const REMEMBER_FILE: &str = "remember.json";

fn remember_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    crate::data_dir(app).map(|d| d.join(REMEMBER_FILE))
}

fn load_remember_config(app: &AppHandle) -> RememberConfig {
    let fallback = RememberConfig {
        enabled: false,
        master_on: false,
        mic: String::new(),
    };
    let Some(p) = remember_path(app) else {
        return fallback;
    };
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return fallback; // 首次运行无配置，属正常
    };
    match serde_json::from_str(&raw) {
        Ok(c) => c,
        Err(e) => {
            log::warn!(target: "config", "remember.json 解析失败，使用默认配置: {e}");
            fallback
        }
    }
}

/// 覆盖写入记住配置：开关状态 + 总开关 + 麦克风（仅开启时记录）
fn save_remember_config(app: &AppHandle) {
    let cfg = {
        let eng = ENGINE.lock().unwrap();
        match eng.as_ref() {
            Some(e) => RememberConfig {
                enabled: e.remember,
                master_on: if e.remember { e.master_on } else { false },
                mic: if e.remember { e.mic_id.clone() } else { String::new() },
            },
            None => return,
        }
    };
    if let Some(p) = remember_path(app) {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string()).and_then(|s| {
            std::fs::write(&p, s).map_err(|e| e.to_string())
        }) {
            Ok(()) => log::info!(target: "config", "remember.json 已覆盖写入: enabled={} master_on={} mic={}", cfg.enabled, cfg.master_on, cfg.mic),
            Err(e) => log::error!(target: "config", "remember.json 写入失败: {e}"),
        }
    }
}

#[tauri::command]
pub fn get_remember(app: AppHandle) -> RememberConfig {
    load_remember_config(&app)
}

#[tauri::command]
pub fn set_remember(on: bool) {
    log::info!(target: "config", "记住选择开关: {on}");
    let mut eng = ENGINE.lock().unwrap();
    if let Some(e) = eng.as_mut() {
        e.remember = on;
    }
}

// ---------- 默认麦克风自动切换持久化：original_mic.json 记录原默认输入设备（第2期 P1） ----------
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OriginalMic {
    id: String,
}

const ORIGINAL_MIC_FILE: &str = "original_mic.json";

fn original_mic_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    crate::data_dir(app).map(|d| d.join(ORIGINAL_MIC_FILE))
}

fn load_original_mic(app: &AppHandle) -> Option<String> {
    let p = original_mic_path(app)?;
    let raw = std::fs::read_to_string(&p).ok()?;
    match serde_json::from_str::<OriginalMic>(&raw) {
        Ok(m) if !m.id.is_empty() => Some(m.id),
        Ok(_) => None,
        Err(e) => {
            log::warn!(target: "mic", "original_mic.json 解析失败，忽略记录: {e}");
            None
        }
    }
}

fn save_original_mic(app: &AppHandle, id: &str) {
    let Some(p) = original_mic_path(app) else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let cfg = OriginalMic { id: id.to_string() };
    if let Err(e) = serde_json::to_string_pretty(&cfg)
        .map_err(|e| e.to_string())
        .and_then(|s| std::fs::write(&p, s).map_err(|e| e.to_string()))
    {
        log::error!(target: "mic", "original_mic.json 写入失败: {e}");
    }
}

fn clear_original_mic(app: &AppHandle) {
    let Some(p) = original_mic_path(app) else { return };
    let _ = std::fs::remove_file(&p);
}

/// 启动钩子：默认非 CABLE 且 CABLE 在线时，记录原默认麦克风并切到 CABLE。
/// 任一步失败仅记日志，不阻塞启动。追溯 TC-031/033/034。
pub fn switch_default_mic_to_cable_on_startup(app: &AppHandle) {
    if !device_enum::detect_virtual_cable() {
        return; // 无 CABLE，不动系统设置（TC-034）
    }
    if device_enum::default_mic_is_cable() {
        log::info!(target: "mic", "默认麦克风已是 CABLE，跳过切换（保留既有记录）");
        return; // TC-033：不重复切换，保留记录供退出回切
    }
    let Some(orig) = device_enum::get_default_capture_id() else {
        log::warn!(target: "mic", "无法获取当前默认输入设备，跳过记录");
        return;
    };
    save_original_mic(app, &orig);
    match device_enum::find_cable_capture_id() {
        Some(cable) => match device_enum::set_default_capture_device(&cable) {
            Ok(()) => log::info!(target: "mic", "默认麦克风已切到 CABLE（原设备已记录）"),
            Err(e) => log::warn!(target: "mic", "切换默认麦克风到 CABLE 失败（记录保留）: {e}"),
        },
        None => log::warn!(target: "mic", "未找到 CABLE 采集端点，跳过切换"),
    }
}

/// 退出钩子：有原设备记录且 CABLE 在线时切回，成功后才清记录。追溯 TC-032/034。
pub fn restore_default_mic_on_exit(app: &AppHandle) {
    let saved = load_original_mic(app);
    let cable_exists = device_enum::detect_virtual_cable();
    if !device_enum::should_restore_mic(saved.as_deref(), cable_exists) {
        return;
    }
    let orig = saved.expect("should_restore_mic 已保证有记录");
    match device_enum::set_default_capture_device(&orig) {
        Ok(()) => {
            clear_original_mic(app);
            log::info!(target: "mic", "默认麦克风已切回原设备，记录已清空");
        }
        Err(e) => log::error!(target: "mic", "回切默认麦克风失败（记录保留，下次启动重试）: {e}"),
    }
}

/// 前端日志通道：Vue 侧异常经此落盘（target=frontend）
#[tauri::command]
pub fn write_log(level: String, message: String) {
    match level.as_str() {
        "error" => log::error!(target: "frontend", "{message}"),
        "warn" => log::warn!(target: "frontend", "{message}"),
        _ => log::info!(target: "frontend", "{message}"),
    }
}

/// 读取收藏列表（第2期 P2）：返回小写 exe 列表。追溯 TC-035。
#[tauri::command]
pub fn get_favorites(app: AppHandle) -> Vec<String> {
    let mut list: Vec<String> = ENGINE
        .lock()
        .unwrap()
        .as_ref()
        .map(|e| e.favorites.iter().cloned().collect())
        .unwrap_or_default();
    list.sort();
    list
}

/// 收藏/取消收藏（第2期 P2）：改内存+文件，emit favorites_changed 同步前端星标。追溯 TC-035。
#[tauri::command]
pub fn set_favorite(app: AppHandle, exe: String, on: bool) {
    let exe = exe.to_lowercase();
    {
        let mut eng = ENGINE.lock().unwrap();
        if let Some(e) = eng.as_mut() {
            if on {
                e.favorites.insert(exe.clone());
            } else {
                e.favorites.remove(&exe);
            }
        }
    }
    let favorites: HashSet<String> = ENGINE
        .lock()
        .unwrap()
        .as_ref()
        .map(|e| e.favorites.clone())
        .unwrap_or_default();
    save_favorites(&app, &favorites);
    let _ = app.emit("favorites_changed", &favorites);
    log::info!(target: "fav", "收藏更新: exe={exe} on={on}");
}

pub fn init_engine(app: AppHandle) {
    let done = load_onboarding_done(&app);
    let cfg = load_remember_config(&app);
    log::info!(target: "app", "引擎初始化: onboarding_done={done} remember={} master_on={} mic={}", cfg.enabled, cfg.master_on, cfg.mic);
    {
        let mut eng = ENGINE.lock().unwrap();
        if eng.is_none() {
            let mut e = Engine::new();
            e.onboarding_done = done;
            e.remember = cfg.enabled;
            // 记忆管理：开启记住选择时恢复上次状态（设备已拔出则 ensure_pipeline 回退）
            if cfg.enabled {
                e.master_on = cfg.master_on;
                e.mic_id = cfg.mic;
            }
            // 收藏应用自动共享名单（第2期 P2）：启动即载入
            e.favorites = load_favorites(&app);
            *eng = Some(e);
        }
    }
    // 启动时检测 CABLE，若在则启动混音链路（TC-006）；锁外完成阻塞初始化
    let _ = ensure_pipeline(app);
}

/// 确保混音管线运行（幂等）。阻塞式初始化在锁外完成，持锁区仅做状态插入。
fn ensure_pipeline(app: AppHandle) -> Result<(), String> {
    {
        let eng = ENGINE.lock().unwrap();
        if let Some(e) = eng.as_ref() {
            if e.sink_running {
                return Ok(());
            }
        }
    }
    if !device_enum::detect_virtual_cable() {
        return Ok(()); // 无虚拟声卡时不启动（引导安装流程负责）
    }
    // 锁外创建麦克风采集（阻塞，上限 5s）。
    // 解析设备 id：记忆/已选 id 仍在线则用之，否则回退首个物理麦克风；
    // 绝不允许空 id 走系统默认端点——默认麦克风必须是 CABLE，采它会形成回路自激
    let wanted_id = {
        let eng = ENGINE.lock().unwrap();
        eng.as_ref().map(|e| e.mic_id.clone()).unwrap_or_default()
    };
    let mics = device_enum::list_capture_devices();
    let mic_id = if mics.iter().any(|m| m.id == wanted_id) {
        wanted_id.clone()
    } else {
        mics.first().map(|m| m.id.clone()).unwrap_or_default()
    };
    let new_mic = if mic_id.is_empty() {
        log::warn!(target: "mic", "无可用物理麦克风，人声静音（共享不受影响）");
        None
    } else {
        match MicCapture::new(&mic_id) {
            Ok(v) => Some(v),
            Err(e) => {
                log::warn!(target: "mic", "device={mic_id} 麦克风采集初始化失败（人声静音，共享不受影响）: {e}");
                None
            }
        }
    };

    let mut eng = ENGINE.lock().unwrap();
    let Some(e) = eng.as_mut() else {
        return Ok(());
    };
    if e.sink_running {
        return Ok(());
    }
    if e.mic.is_none() {
        if let Some((mic, consumer)) = new_mic {
            e.mic = Some(mic);
            e.mic_consumer = Some(consumer);
            e.mic_id = mic_id; // 回退选择同步回引擎，get_status/退出保存均据此
        }
    }
    e.thread_stop = Arc::new(Mutex::new(false));
    let stop = e.thread_stop.clone();
    let thread = std::thread::Builder::new()
        .name("mixer".into())
        .spawn(move || mixer_loop_with_stop(app, stop));
    e.thread = thread.ok();
    e.sink_running = e.thread.is_some();
    Ok(())
}

#[tauri::command]
pub fn get_apps() -> Vec<AudioApp> {
    device_enum::list_audio_apps()
}

#[tauri::command]
pub fn get_mics() -> Vec<MicDevice> {
    device_enum::list_capture_devices()
}

/// 引导第 2 步轮询：系统默认输入设备是否已设为 VB-CABLE
#[tauri::command]
pub fn check_default_mic() -> bool {
    device_enum::default_mic_is_cable()
}

#[tauri::command]
pub fn get_status() -> AppStatus {
    let eng = ENGINE.lock().unwrap();
    let e = eng.as_ref().expect("engine not initialized");
    AppStatus {
        cable_installed: e.sink_running || device_enum::detect_virtual_cable(),
        master_on: e.master_on,
        onboarding_done: e.onboarding_done,
        mic_id: e.mic_id.clone(),
        output_muted: e.output_muted,
        shared: e
            .sources
            .iter()
            .filter(|(_, v)| v.shared) // 仅返回正在共享的源（前端勾选状态）
            .map(|(k, v)| SharedInfo {
                pid: *k,
                exe: v.exe.clone(),
                volume: e.volumes.get(k).copied().unwrap_or(80),
            })
            .collect(),
    }
}

#[tauri::command]
pub async fn toggle_share(app: AppHandle, pid: u32, exe: String, enable: bool) -> Result<(), String> {
    if enable {
        // 检查源是否已存在（可能是之前取消共享但保留了监控流）
        {
            let mut eng = ENGINE.lock().unwrap();
            if let Some(e) = eng.as_mut() {
                if let Some(st) = e.sources.get_mut(&pid) {
                    // 源已存在，重新启用共享
                    log::info!(target: "share", "重新启用共享 pid={pid} exe={exe}");
                    st.shared = true;
                    st.silent_ticks = 0;
                    // 第3期 TC-059：重新启用共享同步悬浮窗图标
                    let _ = app.emit("shared_changed", vec![(pid, true)]);
                    return Ok(());
                }
            }
        }
        // 首次共享时确保混音链路在跑（含 onboarding 后首次共享）
        ensure_pipeline(app.clone())?;
        // 锁外创建进程环回捕获（阻塞，上限 5s）
        let (capture, consumer) = ProcessCapture::new(pid).map_err(|err| {
            log::error!(target: "share", "toggle_share 捕获初始化失败 pid={pid} exe={exe}: {err}");
            err.to_string()
        })?;
        let mut eng = ENGINE.lock().unwrap();
        let e = eng.as_mut().expect("engine not initialized");
        if e.sources.contains_key(&pid) {
            // 并发重复勾选：新捕获随 drop 停止
            let _ = app.emit("shared_changed", vec![(pid, true)]);
            return Ok(());
        }
        e.sources.insert(
            pid,
            SourceState {
                exe: exe.clone(),
                capture,
                consumer,
                level: 0.0,
                shared: true,
                playing_ticks: 0,
                silent_ticks: 0,
                auto_shared: false,
            },
        );
        e.gains.add(pid);
        e.volumes.insert(pid, 80); // 默认80%（PRD）
        e.gains.set_gain(pid, 0.8);
    } else {
        // 取消共享：仅标记不参与混音，捕获流保留以继续监控电平
        let mut eng = ENGINE.lock().unwrap();
        let e = eng.as_mut().expect("engine not initialized");
        if let Some(st) = e.sources.get_mut(&pid) {
            if st.auto_shared {
                // 用户手动取消自动共享的应用：本次会话不再自动重开（TC-038）
                e.auto_opt_out.insert(st.exe.to_lowercase());
                st.auto_shared = false;
                log::info!(target: "fav", "用户手动取消自动共享，本会话不再自动重开 exe={}", st.exe);
            }
            st.playing_ticks = 0;
            st.shared = false;
        }
    }
    // 第3期 TC-059：主界面手动勾选/取消 → 悬浮窗共享图标实时同步
    let _ = app.emit("shared_changed", vec![(pid, enable)]);
    Ok(())
}

/// 带停止标志的混音循环
fn mixer_loop_with_stop(app: AppHandle, stop: Arc<Mutex<bool>>) {
    let sink = match VirtualSink::new() {
        Ok(s) => s,
        Err(e) => {
            log::error!(target: "mixer", "虚拟声卡写出端点打开失败，混音循环未启动: {e}");
            return;
        }
    };
    let mut tick: u64 = 0;
    loop {
        if *stop.lock().unwrap() {
            break;
        }
        if !sink.wait(200) {
            continue;
        }
        // 事件驱动节奏：每 tick 恰好填满一个可用周期，不足补零。
        // 修复：原实现每 tick 弹空队列后整段写入，超出 available 的样本被丢弃、
        // 队列空时不写，导致缓冲干涸 → 静音/数据交替 → 麦克风端持续杂音。
        let available = match sink.available_frames() {
            Ok(n) => n as usize,
            Err(e) => {
                log::error!(target: "mixer", "available_frames 失败，混音循环退出: {e}");
                break;
            }
        };
        if available == 0 {
            continue;
        }
        let need = available * sink.channels as usize;
        let (buffers, mic_buf) = {
            let mut eng = ENGINE.lock().unwrap();
            let Some(e) = eng.as_mut() else { break };
            let mut bufs: Vec<Vec<f32>> = Vec::new();
            for (pid, st) in e.sources.iter_mut() {
                // 始终拉取数据防止环形队列溢出，并计算电平用于监控
                let mut buf = vec![0f32; need];
                let (filled, _) = st.consumer.pop_partial_slice(&mut buf);
                let got = filled.len();
                buf.truncate(got);
                if got > 0 {
                    st.level = rms_level(&buf);
                }
                // 仅当 shared=true 且 master_on=true 时参与混音
                let gain = if st.shared && e.master_on {
                    e.gains.gain(*pid).unwrap_or(1.0)
                } else {
                    0.0
                };
                bufs.push(buf.iter().map(|s| s * gain).collect());
            }
            let mut mic_buf = vec![0f32; need];
            if let Some(mc) = e.mic_consumer.as_mut() {
                let (filled, _) = mc.pop_partial_slice(&mut mic_buf);
                let got = filled.len();
                mic_buf.truncate(got);
            }
            (bufs, mic_buf)
        };
        let mut refs: Vec<(&[f32], f32)> = buffers.iter().map(|b| (b.as_slice(), 1.0f32)).collect();
        refs.push((mic_buf.as_slice(), 1.0f32));
        // 闭麦（第3期 TC-053）：mix 后整帧清零，对方听不到音乐+人声；共享状态不变
        let muted = {
            let eng = ENGINE.lock().unwrap();
            eng.as_ref().map(|e| e.output_muted).unwrap_or(false)
        };
        let mut out = finalize_output(mix(&refs), muted);
        // 补零至整周期：即使源全部静音也持续填充，渲染端无 underflow 间隙
        out.resize(need, 0.0);
        let _ = sink.write(&out);
        // 麦克风实时电平（独立于混音输出）
        let mic_rms = rms_level(&mic_buf);
        tick += 1;
        if tick % 3 == 0 {
            let levels: Vec<(u32, f32)> = {
                let eng = ENGINE.lock().unwrap();
                match eng.as_ref() {
                    Some(e) => e.sources.iter().map(|(k, v)| (*k, v.level)).collect(),
                    None => vec![],
                }
            };
            let _ = app.emit("level", &levels);
            let _ = app.emit("mic_level", mic_rms);
        }
        // 每 100 tick（约 2 秒）清理已退出的进程源，防止内存无限增长
        if tick % 100 == 0 {
            cleanup_dead_sources();
            // 自动为检测到的新音频会话创建监控源（shared=false，仅监控不混音）
            ensure_monitoring();
            // 收藏应用自动共享检测（第2期 P2）：复用监控流电平，零额外 WASAPI 调用
            auto_share_favorites(&app);
        }
    }
}

/// 清理进程已退出的监控源（保留捕获流直到进程终止，而非取消共享时立即移除）
fn cleanup_dead_sources() {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let mut eng = ENGINE.lock().unwrap();
    let Some(e) = eng.as_mut() else { return };
    let dead_pids: Vec<u32> = e
        .sources
        .keys()
        .filter(|&&pid| {
            // 尝试打开进程句柄，失败则进程已退出
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.is_err()
        })
        .copied()
        .collect();
    for pid in dead_pids {
        if let Some(mut st) = e.sources.remove(&pid) {
            st.capture.stop();
        }
        e.gains.remove(pid);
        e.volumes.remove(&pid);
    }
}

/// 自动为检测到的新音频会话创建监控源（shared=false，仅监控不混音）
/// 在混音循环中定期调用，使应用启动时即可监听播放状态与实时音量
fn ensure_monitoring() {
    // 1. 获取当前所有音频会话应用
    let apps = device_enum::list_audio_apps();
    
    // 2. 找出尚未监控的应用（持锁时间极短）
    let new_apps: Vec<_> = {
        let eng = ENGINE.lock().unwrap();
        let Some(e) = eng.as_ref() else { return };
        apps.into_iter()
            .filter(|a| !e.sources.contains_key(&a.pid))
            .collect()
    };
    
    // 3. 为每个新应用创建监控源（锁外执行，避免阻塞混音循环）
    for app_info in new_apps {
        // ProcessCapture::new() 是阻塞操作（最长 5s），在锁外执行
        match ProcessCapture::new(app_info.pid) {
            Ok((capture, consumer)) => {
                let mut eng = ENGINE.lock().unwrap();
                if let Some(e) = eng.as_mut() {
                    // 双重检查：防止创建期间已被其他逻辑插入
                    if !e.sources.contains_key(&app_info.pid) {
                        e.sources.insert(
                            app_info.pid,
                            SourceState {
                                exe: app_info.exe.clone(),
                                capture,
                                consumer,
                                level: 0.0,
                                shared: false, // 仅监控，不参与混音
                                playing_ticks: 0,
                                silent_ticks: 0,
                                auto_shared: false,
                            },
                        );
                        e.gains.add(app_info.pid);
                        e.volumes.insert(app_info.pid, 80); // 默认 80%
                        e.gains.set_gain(app_info.pid, 0.8);
                        log::info!(target: "monitor", "自动开始监控 pid={} exe={}", app_info.pid, app_info.exe);
                    }
                }
            }
            Err(err) => {
                log::warn!(target: "monitor", "创建监控源失败 pid={} exe={}: {}", app_info.pid, app_info.exe, err);
            }
        }
    }
}

#[tauri::command]
pub fn set_volume(pid: u32, volume: u8) {
    let gain = (volume as f32 / 100.0).clamp(0.0, 1.0);
    let mut eng = ENGINE.lock().unwrap();
    if let Some(e) = eng.as_mut() {
        e.volumes.insert(pid, volume);
        if e.gains.gain(pid).is_some() {
            e.gains.set_gain(pid, gain);
        }
    }
}

// ---------- 收藏应用自动共享（第2期 P2）：电平驱动判定纯函数（TDD） ----------

/// 播放判定阈值（RMS，与前端电平条同量级）
const PLAY_THRESHOLD: f32 = 0.005;
/// 连续静音 tick 数判定停止（混音循环每 tick ~2s，即 ~6s 缓冲）
const SILENT_TICKS_STOP: u32 = 3;
/// 连续高电平 tick 数判定播放（~4s，防瞬时噪声）
const PLAY_TICKS_START: u32 = 2;

/// 播放触发判定（纯函数，TDD）：高电平且连续高电平 tick 达标才自动开启共享。追溯 TC-036/040。
pub fn should_auto_start(level: f32, playing_ticks: u32) -> bool {
    level > PLAY_THRESHOLD && playing_ticks >= PLAY_TICKS_START
}

/// 停止触发判定（纯函数，TDD）：静音且连续静音 tick 达标才自动取消共享。追溯 TC-037。
pub fn should_auto_stop(level: f32, silent_ticks: u32) -> bool {
    level <= PLAY_THRESHOLD && silent_ticks >= SILENT_TICKS_STOP
}

/// 播放 tick 累计（纯函数，TDD）：高电平则 +1，否则清零。
pub fn next_playing_ticks(level: f32, playing_ticks: u32) -> u32 {
    if level > PLAY_THRESHOLD {
        playing_ticks.saturating_add(1)
    } else {
        0
    }
}

/// 静音 tick 累计（纯函数，TDD）：静音则 +1，否则清零。
pub fn next_silent_ticks(level: f32, silent_ticks: u32) -> u32 {
    if level <= PLAY_THRESHOLD {
        silent_ticks.saturating_add(1)
    } else {
        0
    }
}

/// 收藏 exe 归一化：小写 + 去重 + 排序（持久化与比较统一口径，与 list_audio_apps 归并键一致）
pub fn normalize_favorite_exes(exes: Vec<String>) -> Vec<String> {
    exes.into_iter()
        .map(|e| e.to_lowercase())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

// ---------- 收藏应用自动共享持久化：favorites.json（第2期 P2） ----------
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FavoritesConfig {
    exes: Vec<String>,
}

const FAVORITES_FILE: &str = "favorites.json";

fn favorites_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    crate::data_dir(app).map(|d| d.join(FAVORITES_FILE))
}

/// 读取收藏（小写 exe 集合）；文件缺失/解析失败回退空集（容错模式，同 remember.json）
fn load_favorites(app: &AppHandle) -> HashSet<String> {
    let Some(p) = favorites_path(app) else {
        return HashSet::new();
    };
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return HashSet::new(); // 首次运行无收藏，属正常
    };
    match serde_json::from_str::<FavoritesConfig>(&raw) {
        Ok(c) => c.exes.into_iter().map(|e| e.to_lowercase()).collect(),
        Err(e) => {
            log::warn!(target: "fav", "favorites.json 解析失败，忽略收藏: {e}");
            HashSet::new()
        }
    }
}

/// 覆盖写入收藏（归一化后落盘）
fn save_favorites(app: &AppHandle, exes: &HashSet<String>) {
    let Some(p) = favorites_path(app) else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let cfg = FavoritesConfig {
        exes: normalize_favorite_exes(exes.iter().cloned().collect()),
    };
    if let Err(e) = serde_json::to_string_pretty(&cfg)
        .map_err(|e| e.to_string())
        .and_then(|s| std::fs::write(&p, s).map_err(|e| e.to_string()))
    {
        log::error!(target: "fav", "favorites.json 写入失败: {e}");
    }
}

/// 收藏应用自动共享检测（第2期 P2）：混音循环每 100 tick 调用一次。
/// 只翻转 shared 标记（复用 ensure_monitoring 已建捕获），不重建捕获、不走 toggle_share。
/// 追溯 TC-036/037/038/039/040。
fn auto_share_favorites(app: &AppHandle) {
    let changed: Vec<(u32, bool)> = {
        let mut eng = ENGINE.lock().unwrap();
        let Some(e) = eng.as_mut() else { return };
        let mut changed = Vec::new();
        for (pid, st) in e.sources.iter_mut() {
            let exe = st.exe.to_lowercase();
            // 仅收藏且未进入会话级退出名单的应用参与自动共享（TC-038/039）
            if !e.favorites.contains(&exe) || e.auto_opt_out.contains(&exe) {
                continue;
            }
            let level = st.level;
            if !st.shared {
                st.playing_ticks = next_playing_ticks(level, st.playing_ticks);
                if should_auto_start(level, st.playing_ticks) {
                    st.shared = true;
                    st.auto_shared = true;
                    st.silent_ticks = 0;
                    // 与手动共享一致：首次自动共享默认 80% 音量，避免 gain 落到混音循环 unwrap_or(1.0)
                    if !e.volumes.contains_key(pid) {
                        e.gains.add(*pid);
                        e.gains.set_gain(*pid, 0.8);
                        e.volumes.insert(*pid, 80);
                    }
                    changed.push((*pid, true));
                    log::info!(target: "fav", "收藏应用自动开始共享 pid={pid} exe={exe} level={level:.4}");
                }
            } else if st.auto_shared {
                st.silent_ticks = next_silent_ticks(level, st.silent_ticks);
                if should_auto_stop(level, st.silent_ticks) {
                    st.shared = false;
                    st.auto_shared = false;
                    st.playing_ticks = 0;
                    changed.push((*pid, false));
                    log::info!(target: "fav", "收藏应用停止播放，自动取消共享 pid={pid} exe={exe}");
                }
            }
        }
        changed
    };
    if !changed.is_empty() {
        // 前端据此同步勾选状态（AppList 勾选点亮/熄灭）
        let _ = app.emit("shared_changed", &changed);
    }
}

#[tauri::command]
pub async fn set_mic(device_id: String) -> Result<(), String> {
    // TC-008：切换麦克风不断流——仅重建 MicCapture，应用源与写出不动。
    // 空 id 意味着系统默认端点（必为 CABLE），采集它会形成回路自激，直接拒绝
    if device_id.is_empty() {
        return Err("必须指定具体麦克风设备（系统默认为 CABLE，不可采集）".into());
    }
    let (mic, consumer) = MicCapture::new(&device_id).map_err(|e| {
        log::error!(target: "mic", "set_mic 失败 device={device_id}: {e}");
        e.to_string()
    })?;
    let mut eng = ENGINE.lock().unwrap();
    if let Some(e) = eng.as_mut() {
        if let Some(mut old) = e.mic.take() {
            old.stop();
        }
        e.mic = Some(mic);
        e.mic_consumer = Some(consumer);
        e.mic_id = device_id;
    }
    Ok(())
}

#[tauri::command]
pub fn toggle_master(app: AppHandle, on: bool) {
    {
        let mut eng = ENGINE.lock().unwrap();
        if let Some(e) = eng.as_mut() {
            e.master_on = on;
        }
    }
    // 同步所有入口（UI 开关/热键/托盘）并刷新托盘文案（TC-018/TC-022）；锁外执行
    let _ = app.emit("master_changed", on);
    crate::rebuild_tray_menu(&app, on);
}

/// 读取当前总开关状态（热键/托盘动作分发用，非命令）
pub fn master_on() -> bool {
    let eng = ENGINE.lock().unwrap();
    eng.as_ref().map(|e| e.master_on).unwrap_or(false)
}

/// 一键闭麦（第3期 TC-053/058）：输出全静音（对方听不到音乐+人声）；
/// 不改变共享状态、不持久化，重启复位。悬浮窗 🎤 按钮调用。
#[tauri::command]
pub fn set_output_mute(app: AppHandle, on: bool) {
    {
        let mut eng = ENGINE.lock().unwrap();
        if let Some(e) = eng.as_mut() {
            e.output_muted = on;
        }
    }
    let _ = app.emit("output_mute_changed", on);
    log::info!(target: "app", "输出闭麦: {on}");
}

/// 全局媒体播放/暂停（第2期 P2）：与热键 Ctrl+Shift+P、托盘"播放/暂停音乐"同一入口函数。追溯 TC-041/042/043/044。
#[tauri::command]
pub fn media_play_pause() {
    hotkeys::media_play_pause();
}

// ---------- 热键配置命令（TC-020 自定义改键 / TC-021 冲突回滚） ----------

/// 读取热键配置（前端设置面板与提醒行展示用）
#[tauri::command]
pub fn get_hotkeys(app: AppHandle) -> HotkeyConfig {
    hotkeys::load_config(&app)
}

/// 保存热键配置：全量格式校验 → 注销旧 → 注册新（失败回滚）→ 落盘 → 通知前端
#[tauri::command]
pub fn set_hotkeys(app: AppHandle, cfg: HotkeyConfig) -> Result<(), String> {
    // 1. 全量格式校验（空串=禁用，跳过校验）
    for (label, s) in [
        ("toggle_share", &cfg.toggle_share),
        ("play_pause", &cfg.play_pause),
        ("prev_track", &cfg.prev_track),
        ("next_track", &cfg.next_track),
    ] {
        if !s.is_empty() && hotkeys::parse_shortcut(s).is_none() {
            return Err(format!("组合键格式无效: {label}={s}"));
        }
    }
    // 2. 注销旧并注册新；任一条失败 → 回滚旧配置并报错，不落盘
    let _ = app.global_shortcut().unregister_all();
    for s in [&cfg.toggle_share, &cfg.play_pause, &cfg.prev_track, &cfg.next_track] {
        let Some(sc) = hotkeys::parse_shortcut(s) else {
            continue;
        };
        if let Err(e) = app.global_shortcut().register(sc) {
            log::error!(target: "hotkey", "set_hotkeys 注册失败（可能被其它程序占用）: {s} {e}");
            hotkeys::register_all(&app); // 回滚：按旧配置重新注册
            return Err(format!("注册失败，组合可能已被其它程序占用: {s}"));
        }
    }
    // 3. 落盘 + 通知前端刷新
    hotkeys::save_config(&app, &cfg);
    let _ = app.emit("hotkeys_changed", &cfg);
    Ok(())
}

/// 录制模式：true 注销全部热键（已注册热键会拦截 keydown，面板收不到按键），
/// false 按当前配置恢复注册（TC-020 按键捕获录制的关键前置）
#[tauri::command]
pub fn set_hotkeys_recording(app: AppHandle, on: bool) {
    if on {
        let _ = app.global_shortcut().unregister_all();
        log::info!(target: "hotkey", "进入录制模式：热键已全部注销");
    } else {
        hotkeys::register_all(&app);
    }
}

#[cfg(test)]
mod master_tests {
    use super::*;

    /// TC-018：引擎初始 master_on=false；翻转后 master_on() 同步可见
    #[test]
    fn master_default_off_and_flip() {
        {
            let mut eng = ENGINE.lock().unwrap();
            *eng = Some(Engine::new());
            assert!(!eng.as_ref().unwrap().master_on, "初始应为关闭");
        }
        assert!(!master_on());
        {
            let mut eng = ENGINE.lock().unwrap();
            eng.as_mut().unwrap().master_on = true;
        }
        assert!(master_on());
        {
            let mut eng = ENGINE.lock().unwrap();
            eng.as_mut().unwrap().master_on = false;
        }
        assert!(!master_on());
    }
}

#[cfg(test)]
mod auto_share_tests {
    use super::*;

    /// TC-036/040：高电平且连续高电平 tick 达标才触发（严格大于阈值）
    #[test]
    fn auto_start_needs_level_and_ticks() {
        assert!(!should_auto_start(0.004, 2)); // 低电平不触发
        assert!(should_auto_start(0.006, 2)); // 高电平 + 2 tick 触发
        assert!(!should_auto_start(0.006, 1)); // 高电平但 tick 不足
        assert!(should_auto_start(0.006, 3)); // 更高 tick 也触发
        assert!(!should_auto_start(0.005, 2)); // 恰等于阈值不触发（> 判定）
    }

    /// TC-037：静音且连续静音 tick 达标才停止（<= 阈值视为静音）
    #[test]
    fn auto_stop_needs_silence_and_ticks() {
        assert!(should_auto_stop(0.004, 3)); // 静音 + 3 tick 停止
        assert!(!should_auto_stop(0.004, 2)); // tick 不足
        assert!(!should_auto_stop(0.006, 3)); // 高电平不停止
        assert!(should_auto_stop(0.005, 3)); // 恰等于阈值视为静音（<=）
    }

    /// tick 累计：对应电平累加、反向电平清零
    #[test]
    fn ticks_accumulate_and_reset() {
        assert_eq!(next_playing_ticks(0.006, 0), 1);
        assert_eq!(next_playing_ticks(0.006, 1), 2);
        assert_eq!(next_playing_ticks(0.004, 5), 0); // 低电平清零
        assert_eq!(next_silent_ticks(0.004, 0), 1);
        assert_eq!(next_silent_ticks(0.004, 2), 3);
        assert_eq!(next_silent_ticks(0.006, 2), 0); // 高电平清零
    }

    /// TC-035：收藏 exe 归一化（小写 + 去重 + 排序），与 list_audio_apps 归并键一致
    #[test]
    fn favorite_exes_normalize_lowercase_dedup_sort() {
        assert_eq!(
            normalize_favorite_exes(vec!["B.EXE".into(), "a.exe".into(), "b.exe".into(), "A.EXE".into()]),
            vec!["a.exe", "b.exe"]
        );
    }
}

#[tauri::command]
pub async fn finish_onboarding(app: AppHandle) {
    // 持久化引导状态（修复：每次启动重复弹向导）
    save_onboarding_done(&app);
    {
        let mut eng = ENGINE.lock().unwrap();
        if let Some(e) = eng.as_mut() {
            e.onboarding_done = true;
        }
    }
    // 后台启动音频管线（MicCapture 初始化可能等待 5s，不阻塞前端）
    let _ = std::thread::spawn(move || {
        let _ = ensure_pipeline(app);
    });
}

/// 自动安装 VB-CABLE（TC-011）：
/// 已装→直接成功；未装→提权运行内置安装器（UAC 一次确认），等待完成后复检。
/// 返回 (是否已装, 错误信息)。UAC 被拒绝时返回具体错误。
#[tauri::command]
pub async fn install_cable(app: AppHandle) -> Result<InstallResult, String> {
    if device_enum::detect_virtual_cable() {
        return Ok(InstallResult {
            installed: true,
            already: true,
            message: "已安装".into(),
        });
    }

    // 定位资源目录中的安装器（打包后为 resources/ 下；dev 回退源目录）
    let resource = app
        .path()
        .resource_dir()
        .ok()
        .map(|d| d.join("resources/VBCABLE_Setup_x64.exe"))
        .filter(|p| p.exists());
    let exe = resource
        .or_else(|| {
            let fallback = std::path::PathBuf::from("src-tauri/resources/VBCABLE_Setup_x64.exe");
            fallback.exists().then_some(fallback)
        })
        .ok_or("未找到内置安装器 VBCABLE_Setup_x64.exe")?;
    run_installer(&exe).await
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallResult {
    pub installed: bool,
    pub already: bool,
    pub message: String,
}

async fn run_installer(exe: &std::path::Path) -> Result<InstallResult, String> {
    // VB-CABLE 安装器必须在 .inf 文件所在目录运行（否则报 "Missing inf file"）
    // 安装器不支持静默模式（-h 无效），需用户在 GUI 中手动确认
    // 安装后需要重启系统或音频服务才能检测设备
    let exe_str = exe.to_string_lossy().replace('\'', "''");
    let mut cmd = std::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-Command",
        &format!(
            "$exeDir = Split-Path -Parent '{exe_str}'; Start-Process -FilePath '{exe_str}' -WorkingDirectory $exeDir -Verb RunAs -Wait",
        ),
    ]);
    let status = cmd
        .status()
        .map_err(|e| {
            log::error!(target: "installer", "启动 VB-CABLE 安装器失败: {e}");
            format!("启动安装器失败：{e}")
        })?;
    if !status.success() {
        log::error!(target: "installer", "安装器未成功退出（UAC 被取消或安装失败）");
        return Err("安装器未完成（UAC 被取消或安装失败）".into());
    }
    // 安装后等待驱动设备就绪（VB-CABLE 官方要求重启，这里尝试等待 10 秒）
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if device_enum::detect_virtual_cable() {
            return Ok(InstallResult {
                installed: true,
                already: false,
                message: "安装成功".into(),
            });
        }
    }
    // 10 秒内未检测到设备，但安装器已正常退出，提示用户重启
    log::warn!(target: "installer", "安装器已退出但 10s 内未检测到 CABLE Input，需重启系统");
    Ok(InstallResult {
        installed: true,
        already: false,
        message: "安装完成，请重启系统后重新打开应用".into(),
    })
}

#[tauri::command]
pub fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    // 退出前覆盖写入记住选择配置（须在移走 Engine 之前，否则读不到共享列表）
    save_remember_config(&app);
    // 第2期 P1：回切默认麦克风，确保切回原麦克风后才结束进程
    restore_default_mic_on_exit(&app);
    log::info!(target: "app", "AudioShare 退出");
    log::logger().flush();
    // 退出死锁修复：不能在持有 ENGINE 锁时 join 混音线程——
    // 混音线程每 tick 需要短暂获取 ENGINE 锁，持锁 join 会互相等待导致卡死。
    // 因此锁内仅移出引擎所有权（ENGINE 置 None），所有 stop/join 在锁外完成。
    let engine = {
        let mut eng = ENGINE.lock().unwrap();
        eng.take()
    };
    if let Some(mut e) = engine {
        for (_, st) in e.sources.iter_mut() {
            st.capture.stop();
        }
        e.sources.clear();
        if let Some(mut m) = e.mic.take() {
            m.stop();
        }
        e.stop_thread();
    }
    app.exit(0);
}

// ---------- 应用图标提取 ----------

/// 获取进程的 exe 路径
fn get_exe_path_from_pid(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::System::ProcessStatus::GetModuleFileNameExW;

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        if handle.is_invalid() {
            return None;
        }

        let mut buffer = [0u16; 1024];
        let len = GetModuleFileNameExW(
            Some(handle),
            None,
            &mut buffer,
        );
        let _ = CloseHandle(handle);

        if len == 0 {
            return None;
        }

        Some(String::from_utf16_lossy(&buffer[..len as usize]))
    }
}

/// 从 exe 文件提取图标并转换为 base64 PNG
fn extract_icon_as_base64(exe_path: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ExtractIconExW;
    use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC, CreateCompatibleDC, CreateDIBSection, SelectObject, DeleteDC, DeleteObject, BITMAPINFOHEADER, BITMAPINFO, DIB_RGB_COLORS, BI_RGB};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, DrawIconEx, DI_NORMAL};

    unsafe {
        // 提取大图标（32x32 或更大）
        let path_wide: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut hicon = windows::Win32::UI::WindowsAndMessaging::HICON::default();
        let count = ExtractIconExW(
            PCWSTR(path_wide.as_ptr()),
            0,
            Some(&mut hicon),  // 大图标
            None,              // 不需要小图标
            1,
        );

        if count == 0 || hicon.is_invalid() {
            return None;
        }

        // 固定渲染尺寸 32x32（适合 UI 显示）
        const ICON_SIZE: i32 = 32;

        // 创建内存 DC
        let hdc_screen = GetDC(None);
        let hdc_mem = CreateCompatibleDC(Some(hdc_screen));

        // 创建 DIB（32x32, 32 位 BGRA）
        let mut bi = BITMAPINFOHEADER::default();
        bi.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.biWidth = ICON_SIZE;
        bi.biHeight = -ICON_SIZE; // 自上而下
        bi.biPlanes = 1;
        bi.biBitCount = 32;
        bi.biCompression = BI_RGB.0 as u32;

        let bitmap_info = BITMAPINFO {
            bmiHeader: bi,
            bmiColors: [Default::default()],
        };

        let mut pixels: *mut u8 = std::ptr::null_mut();
        let hbitmap = CreateDIBSection(
            Some(hdc_mem),
            &bitmap_info,
            DIB_RGB_COLORS,
            &mut pixels as *mut *mut u8 as *mut *mut _,
            None,
            0,
        ).ok()?;

        let old_bitmap = SelectObject(hdc_mem, hbitmap.into());

        // 绘制图标到 DC（缩放到 32x32）
        let _ = DrawIconEx(hdc_mem, 0, 0, hicon, ICON_SIZE, ICON_SIZE, 0, None, DI_NORMAL);

        // 读取像素数据（BGRA -> RGBA）
        let pixel_count = (ICON_SIZE * ICON_SIZE) as usize;
        let mut rgba_data = Vec::with_capacity(pixel_count * 4);

        for i in 0..pixel_count {
            let offset = i * 4;
            rgba_data.push(*pixels.add(offset + 2)); // R
            rgba_data.push(*pixels.add(offset + 1)); // G
            rgba_data.push(*pixels.add(offset));     // B
            rgba_data.push(*pixels.add(offset + 3)); // A
        }

        // 清理 GDI 对象
        SelectObject(hdc_mem, old_bitmap);
        let _ = DeleteObject(hbitmap.into());
        let _ = DeleteDC(hdc_mem);
        let _ = ReleaseDC(None, hdc_screen);
        let _ = DestroyIcon(hicon);

        // 转换为 PNG
        let img = image::RgbaImage::from_raw(ICON_SIZE as u32, ICON_SIZE as u32, rgba_data)?;
        let mut png_data = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png_data), image::ImageFormat::Png).ok()?;

        // 转换为 base64
        use base64::Engine;
        let base64_str = base64::engine::general_purpose::STANDARD.encode(&png_data);
        Some(format!("data:image/png;base64,{}", base64_str))
    }
}

/// 获取应用图标（base64 PNG）
#[tauri::command]
pub fn get_app_icon(pid: u32) -> Option<String> {
    // 获取 exe 路径
    let exe_path = get_exe_path_from_pid(pid)?;

    // 检查缓存
    {
        let cache = ICON_CACHE.lock().unwrap();
        if let Some(icon) = cache.get(&exe_path) {
            return Some(icon.clone());
        }
    }

    // 提取图标
    let icon = extract_icon_as_base64(&exe_path)?;

    // 存入缓存
    {
        let mut cache = ICON_CACHE.lock().unwrap();
        cache.insert(exe_path, icon.clone());
    }

    Some(icon)
}
