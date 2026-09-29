//! Tauri 命令层与音频引擎编排（技术设计§4 commands.rs）
//! 追溯：TC-001~TC-017
//! 混音线程每 tick 短暂持锁拉取各源帧（rtrb 消费需要 &mut），命令层临界区极短，无阻塞风险。
//! 阻塞式初始化（MicCapture/ProcessCapture）一律在锁外完成，避免持锁阻塞导致 UI 卡死。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_store::StoreExt;

use crate::audio::capture::ProcessCapture;
use crate::audio::device_enum::{self, AudioApp, MicDevice};
use crate::audio::mic_capture::MicCapture;
use crate::audio::mixer::{mix, rms_level, SourceTable};
use crate::audio::virtual_dev::VirtualSink;

#[derive(Debug, Clone, Serialize)]
pub struct AppStatus {
    pub cable_installed: bool,
    pub master_on: bool,
    pub onboarding_done: bool,
    pub mic_id: String,
    pub shared: Vec<SharedInfo>,
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

/// 前端日志通道：Vue 侧异常经此落盘（target=frontend）
#[tauri::command]
pub fn write_log(level: String, message: String) {
    match level.as_str() {
        "error" => log::error!(target: "frontend", "{message}"),
        "warn" => log::warn!(target: "frontend", "{message}"),
        _ => log::info!(target: "frontend", "{message}"),
    }
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
            st.shared = false;
        }
    }
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
        let mut out = mix(&refs);
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
pub fn toggle_master(on: bool) {
    let mut eng = ENGINE.lock().unwrap();
    if let Some(e) = eng.as_mut() {
        e.master_on = on;
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
