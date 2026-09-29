//! 物理麦克风采集（技术设计§4 mic_capture.rs）
//! 追溯：TC-008（麦克风切换实时生效、不断流）
//!
//! 线程模型：全部 WASAPI/COM 操作都在 mic-capture 线程内完成（同线程同公寓），
//! 主线程仅通过 std::sync::mpsc 接收初始化结果与帧队列消费者。
//! 注：跨公寓/未初始化 COM 的线程使用接口指针会导致堆损坏（STATUS_HEAP_CORRUPTION）。
//!
//! 格式策略：优先请求固定 48kHz/2ch/32bit float；若设备共享模式不支持
//! （AUDCLNT_E_UNSUPPORTED_FORMAT=0x88890008，如 44.1k/16bit 混合格式的 USB 麦），
//! 回退设备 GetMixFormat 格式并在采集线程内软件转换（位深解码 → 声道映射立体声 →
//! 线性重采样 48k），下游管线始终看到 48k/2ch/f32。

use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, IAudioClient, IAudioCaptureClient, IMMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Threading::WaitForSingleObject;

/// IEEE float 波形子格式 GUID（KSDATAFORMAT_SUBTYPE_IEEE_FLOAT）
const SUBTYPE_IEEE_FLOAT: windows::core::GUID =
    windows::core::GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

/// 管线统一输出格式
const OUT_RATE: u32 = 48000;
const OUT_CHANNELS: u16 = 2;

/// 设备实际格式（共享模式 mix format），用于原始包解码
#[derive(Debug, Clone, Copy)]
struct MicFormat {
    rate: u32,
    channels: u16,
    bits: u16,      // 每样本容器位深
    block_align: u16,
    is_float: bool,
}

impl MicFormat {
    unsafe fn parse(wfx: *const WAVEFORMATEX) -> Self {
        let w = &*wfx;
        let mut is_float = w.wFormatTag == 3; // WAVE_FORMAT_IEEE_FLOAT
        if w.wFormatTag == 0xFFFE {
            // WAVE_FORMAT_EXTENSIBLE：真实编码看 SubFormat（结构 1 字节对齐，须 read_unaligned）
            let ext_ptr = wfx as *const windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE;
            let sub = std::ptr::read_unaligned(std::ptr::addr_of!((*ext_ptr).SubFormat));
            is_float = sub == SUBTYPE_IEEE_FLOAT;
        }
        Self {
            rate: w.nSamplesPerSec.max(1),
            channels: w.nChannels.max(1),
            bits: w.wBitsPerSample,
            block_align: w.nBlockAlign.max(1),
            is_float,
        }
    }
}

/// 软件转换器：原始包字节 → 48kHz/2ch/f32 interleaved
struct MicConverter {
    fmt: MicFormat,
    res_pos: f64,   // 跨包携带的输入帧小数位置（ext 坐标）
    last: [f32; 2], // 上一包末帧，用于跨包插值连续
}

impl MicConverter {
    fn new(fmt: MicFormat) -> Self {
        Self {
            fmt,
            res_pos: 0.0,
            last: [0.0; 2],
        }
    }

    /// 设备格式恰为管线格式时零转换直通
    fn is_passthrough(&self) -> bool {
        self.fmt.rate == OUT_RATE
            && self.fmt.channels == OUT_CHANNELS
            && self.fmt.is_float
            && self.fmt.bits == 32
    }

    fn process(&mut self, raw: &[u8]) -> Vec<f32> {
        if self.is_passthrough() {
            let n = raw.len() / 4;
            let mut out = vec![0f32; n];
            for (i, s) in out.iter_mut().enumerate() {
                *s = f32::from_le_bytes([raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2], raw[i * 4 + 3]]);
            }
            return out;
        }
        let decoded = self.decode(raw);
        let stereo = to_stereo(&decoded, self.fmt.channels);
        self.resample(&stereo)
    }

    /// 位深解码：容器字节 → f32 归一化（保持原声道数 interleaved）
    fn decode(&self, raw: &[u8]) -> Vec<f32> {
        let nch = self.fmt.channels as usize;
        let stride = (self.fmt.block_align as usize / nch).max(1);
        let frames = raw.len() / self.fmt.block_align as usize;
        let mut out = Vec::with_capacity(frames * nch);
        for f in 0..frames {
            for c in 0..nch {
                let off = f * self.fmt.block_align as usize + c * stride;
                if off + stride > raw.len() {
                    break;
                }
                out.push(match (self.fmt.is_float, self.fmt.bits) {
                    (true, 32) => {
                        f32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]])
                    }
                    (false, 32) => {
                        i32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]])
                            as f32
                            / 2_147_483_648.0
                    }
                    (false, 24) => {
                        let v = (raw[off] as i32)
                            | ((raw[off + 1] as i32) << 8)
                            | ((raw[off + 2] as i32) << 16);
                        let v = (v << 8) >> 8; // 符号扩展
                        v as f32 / 8_388_608.0
                    }
                    (false, 16) => i16::from_le_bytes([raw[off], raw[off + 1]]) as f32 / 32768.0,
                    (false, 8) => (raw[off] as i8) as f32 / 128.0,
                    _ => 0.0,
                });
            }
        }
        out
    }

    /// 线性重采样至 48k：ext = [上包末帧, 本包...]，保证跨包插值连续
    fn resample(&mut self, stereo: &[f32]) -> Vec<f32> {
        let in_rate = self.fmt.rate;
        if in_rate == OUT_RATE {
            return stereo.to_vec();
        }
        let n_in = stereo.len() / 2;
        if n_in == 0 {
            return vec![];
        }
        let mut ext = Vec::with_capacity(stereo.len() + 2);
        ext.extend_from_slice(&self.last);
        ext.extend_from_slice(stereo);
        let n_ext = n_in + 1;
        let ratio = in_rate as f64 / OUT_RATE as f64;
        let mut out = Vec::new();
        let mut p = self.res_pos;
        while (p as usize) + 1 < n_ext {
            let k = p as usize;
            let f = (p - k as f64) as f32;
            let (a, b) = (&ext[k * 2..k * 2 + 2], &ext[(k + 1) * 2..(k + 1) * 2 + 2]);
            out.push(a[0] + (b[0] - a[0]) * f);
            out.push(a[1] + (b[1] - a[1]) * f);
            p += ratio;
        }
        self.last = [stereo[(n_in - 1) * 2], stereo[(n_in - 1) * 2 + 1]];
        self.res_pos = (p - (n_ext - 1) as f64).max(0.0);
        out
    }
}

/// 声道映射：单声道复制、多声道取前两路
fn to_stereo(samples: &[f32], nch: u16) -> Vec<f32> {
    match nch {
        1 => samples.iter().flat_map(|s| [*s, *s]).collect(),
        2 => samples.to_vec(),
        n => samples.chunks(n as usize).flat_map(|c| [c[0], c[1]]).collect(),
    }
}

/// 麦克风采集会话：推 48k/2ch f32 帧入环形队列
pub struct MicCapture {
    pub sample_rate: u32,
    pub channels: u16,
    stop_flag: Arc<Mutex<bool>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// 采集线程初始化结果：(采样率, 声道数, 帧队列消费者)——恒为管线格式 48k/2ch
type SetupResult = windows::core::Result<(u32, u16, rtrb::Consumer<f32>)>;

impl MicCapture {
    /// device_id 为空串时使用系统默认设备。
    /// 阻塞至采集线程完成设备打开与流启动（成功回传消费者或失败回传错误）。
    pub fn new(device_id: &str) -> windows::core::Result<(Self, rtrb::Consumer<f32>)> {
        let device_id = device_id.to_string();
        let stop_flag = Arc::new(Mutex::new(false));
        let stop_clone = stop_flag.clone();
        let (tx, rx) = mpsc::channel::<SetupResult>();
        // 线程持有独立副本：外层 device_id 保留用于失败日志
        let device_id_for_thread = device_id.clone();
        let thread = std::thread::Builder::new()
            .name("mic-capture".into())
            .spawn(move || mic_capture_thread(stop_clone, device_id_for_thread, tx))?;
        // 等待初始化结果（上限 5s；超时则置停止标志并返回错误，防止线程泄漏）
        let (rate, channels, consumer) = match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                *stop_flag.lock().unwrap() = true;
                return Err(e);
            }
            Err(_) => {
                log::error!(target: "mic", "device={device_id} 麦克风采集初始化超时（>5s）");
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
        let (client, capture, event, mut producer, consumer, mut conv) =
            match open_mic(&device_id) {
                Ok(t) => t,
                Err(e) => {
                    log::error!(target: "mic", "device={device_id} 麦克风打开失败: {e}");
                    let _ = tx.send(Err(e));
                    return;
                }
            };
        // —— 回传消费者与格式（主线程阻塞等待此处） ——
        let _ = tx.send(Ok((OUT_RATE, OUT_CHANNELS, consumer)));

        // —— 采集主循环：事件驱动，200ms 超时检查停止标志 ——
        let block_align = conv.fmt.block_align as usize;
        loop {
            if *stop.lock().unwrap() {
                break;
            }
            if WaitForSingleObject(event, 200) != WAIT_OBJECT_0 {
                continue;
            }
            let mut packet_size = match capture.GetNextPacketSize() {
                Ok(n) => n,
                Err(e) => {
                    log::warn!(target: "mic", "GetNextPacketSize 失败，采集循环退出: {e}");
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
                    let raw = std::slice::from_raw_parts(data, nframes as usize * block_align);
                    for s in conv.process(raw) {
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
/// 优先固定 48kHz/2ch/f32；设备不支持时回退 mix format + 软件转换（见模块头注释）。
unsafe fn open_mic(
    device_id: &str,
) -> windows::core::Result<(
    IAudioClient,
    IAudioCaptureClient,
    windows::Win32::Foundation::HANDLE,
    rtrb::Producer<f32>,
    rtrb::Consumer<f32>,
    MicConverter,
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
    let mix_ptr = client.GetMixFormat()?;
    let mix_fmt = MicFormat::parse(mix_ptr);

    let ext = super::fixed_float_format();
    let fmt = &ext.Format;
    // 首选固定格式；AUDCLNT_E_UNSUPPORTED_FORMAT 等设备回退 mix format
    let actual = match client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
        200_000, // 20ms
        0,
        fmt,
        None,
    ) {
        Ok(()) => MicFormat {
            rate: OUT_RATE,
            channels: OUT_CHANNELS,
            bits: 32,
            block_align: 8,
            is_float: true,
        },
        Err(e) => {
            log::warn!(target: "mic", "device={device_id} 固定格式不支持({e})，回退 mix format 软件转换: {mix_fmt:?}");
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                200_000,
                0,
                mix_ptr,
                None,
            )?;
            // 埋点：若后续再现“初始化超时”，据此区分卡在 Initialize 还是 Start
            log::info!(target: "mic", "device={device_id} mix format Initialize 完成");
            mix_fmt
        }
    };
    // GetMixFormat 内存须用 CoTaskMemFree 释放（误用 Box::from_raw 会堆损坏）
    CoTaskMemFree(Some(mix_ptr as *mut _));

    let event = windows::Win32::System::Threading::CreateEventW(None, false, false, None)?;
    client.SetEventHandle(event)?;
    let capture: IAudioCaptureClient = client.GetService()?;
    client.Start()?;
    log::info!(target: "mic", "device={device_id} 采集流已启动，实际格式: {actual:?}");

    let (producer, consumer) =
        rtrb::RingBuffer::<f32>::new(OUT_RATE as usize * OUT_CHANNELS as usize / 10);
    Ok((
        client,
        capture,
        event,
        producer,
        consumer,
        MicConverter::new(actual),
    ))
}
