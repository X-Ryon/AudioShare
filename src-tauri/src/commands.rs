//! Tauri 命令层与音频引擎编排（技术设计§4 commands.rs）
//! 追溯：TC-001~TC-017
//! 混音线程每 tick 短暂持锁拉取各源帧（rtrb 消费需要 &mut），命令层临界区极短，无阻塞风险。
//! 阻塞式初始化（MicCapture/ProcessCapture）一律在锁外完成，避免持锁阻塞导致 UI 卡死。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
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
            master_on: true,
            onboarding_done: false,
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

fn load_onboarding_done(app: &AppHandle) -> bool {
    app.store(SETTINGS_FILE)
        .ok()
        .and_then(|s| s.get("onboarding_done"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn save_onboarding_done(app: &AppHandle) {
    if let Ok(store) = app.store(SETTINGS_FILE) {
        store.set("onboarding_done", json!(true));
        let _ = store.save();
    }
}

pub fn init_engine(app: AppHandle) {
    let done = load_onboarding_done(&app);
    {
        let mut eng = ENGINE.lock().unwrap();
        if eng.is_none() {
            let mut e = Engine::new();
            e.onboarding_done = done;
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
    // 锁外创建麦克风采集（阻塞，上限 5s）
    let mic_id = {
        let eng = ENGINE.lock().unwrap();
        eng.as_ref().map(|e| e.mic_id.clone()).unwrap_or_default()
    };
    let new_mic = MicCapture::new(&mic_id).ok();

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
        {
            let eng = ENGINE.lock().unwrap();
            if let Some(e) = eng.as_ref() {
                if e.sources.contains_key(&pid) {
                    return Ok(());
                }
            }
        }
        // 首次共享时确保混音链路在跑（含 onboarding 后首次共享）
        ensure_pipeline(app.clone())?;
        // 锁外创建进程环回捕获（阻塞，上限 5s）
        let (capture, consumer) = ProcessCapture::new(pid).map_err(|err| err.to_string())?;
        let mut eng = ENGINE.lock().unwrap();
        let e = eng.as_mut().expect("engine not initialized");
        if e.sources.contains_key(&pid) {
            return Ok(()); // 并发重复勾选：新捕获随 drop 停止
        }
        e.sources.insert(
            pid,
            SourceState {
                exe: exe.clone(),
                capture,
                consumer,
                level: 0.0,
            },
        );
        e.gains.add(pid);
        e.volumes.insert(pid, 80); // 默认80%（PRD）
        e.gains.set_gain(pid, 0.8);
    } else {
        let mut eng = ENGINE.lock().unwrap();
        let e = eng.as_mut().expect("engine not initialized");
        if let Some(mut st) = e.sources.remove(&pid) {
            st.capture.stop();
        }
        e.gains.remove(pid);
        e.volumes.remove(&pid);
    }
    Ok(())
}

/// 带停止标志的混音循环
fn mixer_loop_with_stop(app: AppHandle, stop: Arc<Mutex<bool>>) {
    let sink = match VirtualSink::new() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut tick: u64 = 0;
    loop {
        if *stop.lock().unwrap() {
            break;
        }
        if !sink.wait(200) {
            continue;
        }
        let (buffers, mic_buf) = {
            let mut eng = ENGINE.lock().unwrap();
            let Some(e) = eng.as_mut() else { break };
            let mut bufs: Vec<Vec<f32>> = Vec::new();
            for (pid, st) in e.sources.iter_mut() {
                let mut buf: Vec<f32> = Vec::new();
                while let Ok(s) = st.consumer.pop() {
                    buf.push(s);
                }
                if buf.is_empty() {
                    continue;
                }
                st.level = rms_level(&buf);
                let gain = if e.master_on {
                    e.gains.gain(*pid).unwrap_or(1.0)
                } else {
                    0.0
                };
                bufs.push(buf.iter().map(|s| s * gain).collect());
            }
            let mut mic_buf: Vec<f32> = Vec::new();
            if let Some(mc) = e.mic_consumer.as_mut() {
                while let Ok(s) = mc.pop() {
                    mic_buf.push(s);
                }
            }
            (bufs, mic_buf)
        };
        let mut refs: Vec<(&[f32], f32)> = buffers.iter().map(|b| (b.as_slice(), 1.0f32)).collect();
        refs.push((mic_buf.as_slice(), 1.0f32));
        let out = mix(&refs);
        if !out.is_empty() {
            let _ = sink.write(&out);
        }
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

#[tauri::command]
pub async fn set_mic(device_id: String) -> Result<(), String> {
    // TC-008：切换麦克风不断流——仅重建 MicCapture，应用源与写出不动
    let (mic, consumer) = MicCapture::new(&device_id).map_err(|e| e.to_string())?;
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
        .map_err(|e| format!("启动安装器失败：{e}"))?;
    if !status.success() {
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
