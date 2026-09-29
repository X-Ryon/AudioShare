//! 按进程环回捕获（技术设计§4 capture.rs）
//! WASAPI Process Loopback（经真机诊断验证的正确姿势）：
//!   1. 手工构造 VT_BLOB PROPVARIANT（pBlobData 指向 params 本体，非 InitPropVariantFromBuffer）
//!   2. 自定格式 48k/16bit/2ch PCM（不支持 GetMixFormat）
//!   3. Initialize 带 AUDCLNT_STREAMFLAGS_LOOPBACK，时长参数为 0
//!   4. 轮询模式采集（不支持 EVENTCALLBACK）
//! 线程模型：激活 + 初始化 + 采集全部在专用 capture 线程内完成（同线程同公寓），
//! 主线程仅通过 mpsc 接收初始化结果与帧队列消费者。
//! 注：跨公寓/未初始化 COM 的线程使用接口指针会导致堆损坏（STATUS_HEAP_CORRUPTION）。
//! 追溯：TC-001（按应用取声）、TC-004（Chrome 进程树聚合）

use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

use windows::core::{Interface, PCWSTR};
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioClient, IAudioCaptureClient, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::BLOB;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Variant::VT_BLOB;

/// 进程环回虚拟设备接口名 "VAD\Process_Loopback"
const VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK: PCWSTR = windows::core::w!("VAD\x5CProcess_Loopback");

type ClientResult = windows::core::Result<IAudioClient>;

#[windows::core::implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivateHandler {
    done: Arc<(Mutex<bool>, Condvar)>,
    result: Arc<Mutex<Option<ClientResult>>>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivateHandler_Impl {
    fn ActivateCompleted(
        &self,
        operation: windows::core::Ref<'_, IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let op = operation
            .ok()
            .map_err(|_| windows::core::Error::from(windows::core::HRESULT(-1)))?;
        let mut hr = windows::core::HRESULT(0);
        let mut unk: Option<windows::core::IUnknown> = None;
        unsafe { op.GetActivateResult(&mut hr, &mut unk) }?;
        let client = match unk {
            Some(u) => u.cast::<IAudioClient>(),
            None => Err(hr.into()),
        };
        *self.result.lock().unwrap() = Some(client);
        let (lock, cvar) = &*self.done;
        *lock.lock().unwrap() = true;
        cvar.notify_one();
        Ok(())
    }
}

/// 等待异步激活完成，返回 IAudioClient（调用线程即 capture 线程，同公寓）
fn activate_process_loopback(pid: u32) -> ClientResult {
    super::device_enum::ensure_com();
    unsafe {
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };

        // 官方 C++ 示例写法：VT_BLOB + pBlobData 指向 params 本体
        let blob = BLOB {
            cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            pBlobData: &params as *const _ as *mut u8,
        };
        // 关键：PROPVARIANT 的 Drop 会调用 PropVariantClear，对 VT_BLOB 会 CoTaskMemFree(pBlobData)。
        // pBlobData 指向栈上 params（官方示例语义，非 COM 分配），必须用 ManuallyDrop 阻止自动清理，
        // 否则 CoTaskMemFree(栈指针) → STATUS_HEAP_CORRUPTION。
        let mut propvariant: std::mem::ManuallyDrop<PROPVARIANT> =
            std::mem::ManuallyDrop::new(std::mem::zeroed());
        {
            let inner = &mut *propvariant.Anonymous.Anonymous;
            inner.vt = VT_BLOB;
            let p_blob: *mut BLOB = std::ptr::addr_of_mut!(inner.Anonymous.blob);
            p_blob.write_unaligned(blob);
        }

        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let result_slot: Arc<Mutex<Option<ClientResult>>> = Arc::new(Mutex::new(None));
        let handler: IActivateAudioInterfaceCompletionHandler = ActivateHandler {
            done: done.clone(),
            result: result_slot.clone(),
        }
        .into();

        let _op: IActivateAudioInterfaceAsyncOperation = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&*propvariant as *const PROPVARIANT),
            &handler,
        )?;

        // 等待完成
        let (lock, cvar) = &*done;
        let mut guard = lock.lock().unwrap();
        while !*guard {
            guard = cvar.wait(guard).unwrap();
        }
        let taken = result_slot
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| {
                Err(windows::core::Error::from(windows::core::HRESULT(
                    0x80004005u32 as i32,
                )))
            });
        taken
    }
}

/// Process Loopback 固定格式：48kHz / 16bit / 2ch PCM
fn loopback_format() -> WAVEFORMATEX {
    let mut fmt: WAVEFORMATEX = unsafe { std::mem::zeroed() };
    fmt.wFormatTag = 1u16; // WAVE_FORMAT_PCM
    fmt.nSamplesPerSec = 48000;
    fmt.wBitsPerSample = 16;
    fmt.nChannels = 2;
    fmt.nBlockAlign = fmt.nChannels * fmt.wBitsPerSample / 8;
    fmt.nAvgBytesPerSec = fmt.nSamplesPerSec * fmt.nBlockAlign as u32;
    fmt
}

/// 采集线程初始化结果：(采样率, 声道数, 帧队列消费者)
type SetupResult = windows::core::Result<(u32, u16, rtrb::Consumer<f32>)>;

/// 进程捕获会话：捕获线程轮询采集 i16 PCM 并转 f32 推入环形队列
pub struct ProcessCapture {
    pub pid: u32,
    pub sample_rate: u32,
    pub channels: u16,
    stop_flag: Arc<Mutex<bool>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ProcessCapture {
    /// 为目标进程创建捕获，返回 (会话, 帧队列消费者)。
    /// 激活与采集全部在专用线程完成；阻塞至初始化结果返回（上限 5s）。
    pub fn new(pid: u32) -> windows::core::Result<(Self, rtrb::Consumer<f32>)> {
        let stop_flag = Arc::new(Mutex::new(false));
        let stop_clone = stop_flag.clone();
        let (tx, rx) = mpsc::channel::<SetupResult>();
        let thread = std::thread::Builder::new()
            .name(format!("capture-{pid}"))
            .spawn(move || capture_thread(pid, stop_clone, tx))?;
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok((rate, channels, consumer))) => Ok((
                Self {
                    pid,
                    sample_rate: rate,
                    channels,
                    stop_flag,
                    thread: Some(thread),
                },
                consumer,
            )),
            Ok(Err(e)) => {
                // 线程已回传错误并将退出；置停止标志兜底
                *stop_flag.lock().unwrap() = true;
                Err(e)
            }
            Err(_) => {
                // 初始化挂起/线程退出未回传：置停止标志防止线程泄漏
                log::error!(target: "capture", "pid={pid} 捕获初始化超时（>5s）");
                *stop_flag.lock().unwrap() = true;
                Err(windows::core::Error::from(windows::core::HRESULT(
                    0x80004005u32 as i32,
                )))
            }
        }
    }

    pub fn stop(&mut self) {
        *self.stop_flag.lock().unwrap() = true;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ProcessCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 采集线程体：本线程内完成激活、初始化与轮询采集（COM 已初始化为 MTA）
fn capture_thread(pid: u32, stop: Arc<Mutex<bool>>, tx: mpsc::Sender<SetupResult>) {
    super::device_enum::ensure_com();
    let diag = std::env::var_os("AR_DIAG").is_some();
    if diag {
        eprintln!("[cap-{pid}] thread start, com ok");
    }
    unsafe {
        // —— 激活与流初始化（本线程内，同公寓） ——
        let opened = (|| -> windows::core::Result<(
            IAudioClient,
            IAudioCaptureClient,
            u16,
            u32,
            rtrb::Producer<f32>,
            rtrb::Consumer<f32>,
        )> {
            if diag {
                eprintln!("[cap-{pid}] activating...");
            }
            let client = activate_process_loopback(pid)?;
            if diag {
                eprintln!("[cap-{pid}] activated ok");
            }
            let fmt = loopback_format();
            let rate = fmt.nSamplesPerSec;
            let channels = fmt.nChannels;

            // Process Loopback：时长参数必须为 0，带 LOOPBACK 标志
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                0,
                0,
                &fmt,
                None,
            )?;
            if diag {
                eprintln!("[cap-{pid}] initialize ok");
            }
            let capture: IAudioCaptureClient = client.GetService()?;
            if diag {
                eprintln!("[cap-{pid}] getservice ok");
            }
            client.Start()?;
            if diag {
                eprintln!("[cap-{pid}] start ok");
            }

            // 200ms 容量环形队列（轮询周期 50ms，留足余量）
            let (producer, consumer) =
                rtrb::RingBuffer::<f32>::new(rate as usize * channels as usize / 5);
            Ok((client, capture, channels, rate, producer, consumer))
        })();

        let (client, capture, channels, rate, mut producer, consumer) = match opened {
            Ok(t) => t,
            Err(e) => {
                if diag {
                    eprintln!("[cap-{pid}] init FAIL {e:?}");
                }
                log::error!(target: "capture", "pid={pid} 捕获初始化失败: {e:?}");
                let _ = tx.send(Err(e));
                return;
            }
        };
        // —— 回传消费者与格式（主线程阻塞等待此处） ——
        if diag {
            eprintln!("[cap-{pid}] init done, sending consumer");
        }
        let _ = tx.send(Ok((rate, channels, consumer)));

        // —— 轮询采集主循环：GetNextPacketSize/GetBuffer（50ms 周期） ——
            loop {
                if *stop.lock().unwrap() {
                    break;
                }
                // 轮询间隔 10ms：与渲染周期同量级平滑供给，避免长间隔 burst 推送
                // 导致混音端取数不均（队列空时补零、溢出时丢弃 → 波形断裂杂音）
                std::thread::sleep(std::time::Duration::from_millis(10));
                let mut packet_size = match capture.GetNextPacketSize() {
                    Ok(n) => n,
                    Err(_) => {
                        if diag {
                            eprintln!("[cap-{pid}] GetNextPacketSize err, break");
                        }
                        break;
                    }
                };
            while packet_size > 0 {
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut nframes = 0u32;
                let mut flags: u32 = 0;
                if capture
                    .GetBuffer(&mut data, &mut nframes, &mut flags, None, None)
                    .is_err()
                {
                    break;
                }
                if flags != AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 && !data.is_null() {
                    // i16 PCM → f32
                    let samples = std::slice::from_raw_parts(
                        data as *const i16,
                        nframes as usize * channels as usize,
                    );
                    for &s16 in samples {
                        let v = (s16 as f32) / 32768.0;
                        // 队列满丢新样本（消费周期10ms，队列200ms容量，充足）
                        let _ = producer.push(v);
                    }
                }
                let _ = capture.ReleaseBuffer(nframes);
                match capture.GetNextPacketSize() {
                    Ok(n) => packet_size = n,
                    Err(_) => break,
                }
            }
        }
        let _ = client.Stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "需要真实进程在播放音频，集成验证时手动执行"]
    fn capture_real_process() {
        // TC-001 集成自测：对指定 PID 捕获并断言有样本
    }
}
