//! Tauri 命令层与音频引擎编排（技术设计§4 commands.rs）
//! 追溯：TC-001~TC-017
//! 混音线程每 tick 短暂持锁拉取各源帧（rtrb 消费需要 &mut），命令层临界区极短，无阻塞风险。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

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

pub fn init_engine(app: AppHandle) {
    let mut eng = ENGINE.lock().unwrap();
    if eng.is_none() {
        let mut e = Engine::new();
        // 启动时检测 CABLE，若在则启动混音链路（TC-006）
        if device_enum::detect_virtual_cable() {
            start_pipeline(&mut e, app);
        }
        *eng = Some(e);
    }
}

/// 建立麦克风采集 + 混音线程 + 虚拟麦写出（技术设计§3 主流程）
fn start_pipeline(e: &mut Engine, app: AppHandle) {
    start_pipeline_locked(e, app);
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
pub fn toggle_share(app: AppHandle, pid: u32, exe: String, enable: bool) -> Result<(), String> {
    let mut eng = ENGINE.lock().unwrap();
    let e = eng.as_mut().expect("engine not initialized");
    if enable {
        if e.sources.contains_key(&pid) {
            return Ok(());
        }
        // 首次共享时确保混音链路在跑（含 onboarding 后首次共享）
        if !e.sink_running {
            start_pipeline_locked(e, app.clone());
        }
        let (capture, consumer) = ProcessCapture::new(pid).map_err(|err| err.to_string())?;
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
        if let Some(mut st) = e.sources.remove(&pid) {
            st.capture.stop();
        }
        e.gains.remove(pid);
        e.volumes.remove(&pid);
    }
    Ok(())
}

/// 在已持锁上下文中启动管线（start_pipeline 的锁内版本）
fn start_pipeline_locked(e: &mut Engine, app: AppHandle) {
    if e.sink_running || !device_enum::detect_virtual_cable() {
        return;
    }
    if e.mic.is_none() {
        if let Ok((mic, consumer)) = MicCapture::new(&e.mic_id) {
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
    e.sink_running = true;
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
pub fn set_mic(device_id: String) -> Result<(), String> {
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
pub fn finish_onboarding(app: AppHandle) {
    let mut eng = ENGINE.lock().unwrap();
    if let Some(e) = eng.as_mut() {
        e.onboarding_done = true;
        if !e.sink_running {
            start_pipeline_locked(e, app);
        }
    }
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    {
        let mut eng = ENGINE.lock().unwrap();
        if let Some(e) = eng.as_mut() {
            for (_, st) in e.sources.iter_mut() {
                st.capture.stop();
            }
            e.sources.clear();
            if let Some(mut m) = e.mic.take() {
                m.stop();
            }
            e.stop_thread();
        }
    }
    app.exit(0);
}
