//! 物理麦克风采集（技术设计§4 mic_capture.rs）
//! 追溯：TC-008（麦克风切换实时生效、不断流）
//!
//! 线程模型：全部 WASAPI/COM 操作都在 mic-capture 线程内完成（同线程同公寓），
//! 主线程仅通过 std::sync::mpsc 接收初始化结果与帧队列消费者。
//! 注：跨公寓/未初始化 COM 的线程使用接口指针会导致堆损坏（STATUS_HEAP_CORRUPTION）。
//!
//! 格式：请求固定 48kHz/2ch/32bit float（见 audio::fixed_float_format），
//! 共享模式下由音频引擎自动重采样/转声道，消除设备默认格式差异导致的失真。

use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{
    IAudioClient, IAudioCaptureClient, IMMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_SHAREMODE_SHARED, eCapture, eConsole,
};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::Foundation::WAIT_OBJECT_0;

/// 麦克风采集会话：推 f32 帧入环形队列
pub struct MicCapture {
    pub sample_rate: u32,
    pub channels: u16,
    stop_flag: Arc<Mutex<bool>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// 采集线程初始化结果：(采样率, 声道数, 帧队列消费者)
type SetupResult = windows::core::Result<(u32, u16, rtrb::Consumer<f32>)>;

impl MicCapture {
    /// device_id 为空串时使用系统默认设备。
    /// 阻塞至采集线程完成设备打开与流启动（成功回传消费者或失败回传错误）。
    pub fn new(device_id: &str) -> windows::core::Result<(Self, rtrb::Consumer<f32>)> {
        let device_id = device_id.to_string();
        let stop_flag = Arc::new(Mutex::new(false));
        let stop_clone = stop_flag.clone();
        let (tx, rx) = mpsc::channel::<SetupResult>();
        let thread = std::thread::Builder::new()
            .name("mic-capture".into())
            .spawn(move || mic_capture_thread(stop_clone, device_id, tx))?;
        // 等待初始化结果（上限 5s；超时则置停止标志并返回错误，防止线程泄漏）
        let (rate, channels, consumer) = match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                *stop_flag.lock().unwrap() = true;
                return Err(e);
            }
            Err(_) => {
                *stop_flag.lock().unwrap() = true;
                return Err(windows::core::Error::from(windows::core::HRESULT(0x80004005u32 as i32)));
            }
        };
        Ok((
            Self {
                sample_rate: rate,
                channels,
                stop_flag,
                thread: Some(thread),
            },
            consumer,
        ))
    }

    pub fn stop(&mut self) {
        *self.stop_flag.lock().unwrap() = true;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for MicCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 采集线程体：本线程内完成全部 WASAPI 操作（COM 已由 ensure_com 初始化为 MTA）
fn mic_capture_thread(stop: Arc<Mutex<bool>>, device_id: String, tx: mpsc::Sender<SetupResult>) {
    super::device_enum::ensure_com();
    unsafe {
        // —— 设备打开与流初始化（本线程内，同公寓） ——
        let (client, capture, event, mut producer, consumer, rate, channels) =
            match open_mic(&device_id) {
                Ok(t) => t,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
        // —— 回传消费者与格式（主线程阻塞等待此处） ——
        let _ = tx.send(Ok((rate, channels, consumer)));

        // —— 采集主循环：事件驱动，50ms 超时检查停止标志 ——
        loop {
            if *stop.lock().unwrap() {
                break;
            }
            if WaitForSingleObject(event, 200) != WAIT_OBJECT_0 {
                continue;
            }
            let mut packet_size = match capture.GetNextPacketSize() {
                Ok(n) => n,
                Err(_) => break,
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
                    let samples = std::slice::from_raw_parts(
                        data as *const f32,
                        nframes as usize * channels as usize,
                    );
                    for &s in samples {
                        let _ = producer.push(s);
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

/// 打开设备并启动采集流（调用线程即采集线程）。
/// 请求固定 48kHz/2ch/f32 格式，共享模式下引擎自动转换，保证与管线一致。
unsafe fn open_mic(
    device_id: &str,
) -> windows::core::Result<(
    IAudioClient,
    IAudioCaptureClient,
    windows::Win32::Foundation::HANDLE,
    rtrb::Producer<f32>,
    rtrb::Consumer<f32>,
    u32,
    u16,
)> {
    let enumerator: IMMDeviceEnumerator = super::device_enum::new_device_enumerator()?;
    let dev = if device_id.is_empty() {
        enumerator.GetDefaultAudioEndpoint(eCapture, eConsole)?
    } else {
        let mut buf: Vec<u16> = device_id.encode_utf16().collect();
        buf.push(0);
        enumerator.GetDevice(PCWSTR(buf.as_ptr()))?
    };

    let client: IAudioClient = dev.Activate(windows::Win32::System::Com::CLSCTX_ALL, None)?;
    let ext = super::fixed_float_format();
    let fmt = &ext.Format;
    let rate = fmt.nSamplesPerSec;
    let channels = fmt.nChannels;

    client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
        200_000, // 20ms
        0,
        fmt,
        None,
    )?;
    let event = windows::Win32::System::Threading::CreateEventW(None, false, false, None)?;
    client.SetEventHandle(event)?;
    let capture: IAudioCaptureClient = client.GetService()?;
    client.Start()?;

    let (producer, consumer) =
        rtrb::RingBuffer::<f32>::new(rate as usize * channels as usize / 10);
    Ok((client, capture, event, producer, consumer, rate, channels))
}
