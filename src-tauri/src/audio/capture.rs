//! 按进程环回捕获（技术设计§4 capture.rs）
//! WASAPI Process Loopback：AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK
//! 追溯：TC-001（按应用取声）、TC-004（Chrome 进程树聚合）

use std::sync::{Arc, Condvar, Mutex};
use windows::core::{Interface, PCWSTR};
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioClient, IAudioCaptureClient, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_SHAREMODE_SHARED,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::StructuredStorage::{
    InitPropVariantFromBuffer, PROPVARIANT,
};
use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};

/// WAVE_FORMAT_EXTENSIBLE 格式标签（0xFFFE）
const WAVE_FORMAT_EXTENSIBLE_TAG: u16 = 0xFFFE;
/// KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
const SUBTYPE_IEEE_FLOAT: u128 = 0x00000003_0000_0010_8000_00aa00389b71;

/// 跨线程传递 COM 接口的 Send 包装（COM 接口引用计数线程安全）
struct SendPtr<T>(T);
unsafe impl<T> Send for SendPtr<T> {}

/// 进程环回虚拟设备接口名
const VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK: PCWSTR = windows::core::w!("VAD\\Process_Loopback");

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
            None => Err(windows::core::Error::from(hr)),
        };
        *self.result.lock().unwrap() = Some(client);
        let (lock, cvar) = &*self.done;
        *lock.lock().unwrap() = true;
        cvar.notify_one();
        Ok(())
    }
}

/// 等待异步激活完成，返回 IAudioClient
fn activate_process_loopback(pid: u32) -> ClientResult {
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

        let propvariant = InitPropVariantFromBuffer(
            &params as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
        )?;

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
            Some(&propvariant as *const PROPVARIANT),
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
            .unwrap_or_else(|| Err(windows::core::Error::from(windows::core::HRESULT(0x80004005u32 as i32))));
        taken
    }
}

/// WAVEFORMATEX 解析为 (采样率, 声道数, 是否float)
pub fn wave_format_info(fmt: &WAVEFORMATEX) -> (u32, u16, bool) {
    unsafe {
        if fmt.wFormatTag == WAVE_FORMAT_EXTENSIBLE_TAG {
            let ext = &*(fmt as *const WAVEFORMATEX as *const WAVEFORMATEXTENSIBLE);
            let sub: windows::core::GUID = std::ptr::read_unaligned(&raw const ext.SubFormat);
            let is_float = sub == windows::core::GUID::from_u128(SUBTYPE_IEEE_FLOAT);
            (fmt.nSamplesPerSec, fmt.nChannels, is_float)
        } else {
            (fmt.nSamplesPerSec, fmt.nChannels, false)
        }
    }
}

/// 进程捕获会话：捕获线程持续将 f32 帧推入环形队列
pub struct ProcessCapture {
    pub pid: u32,
    pub sample_rate: u32,
    pub channels: u16,
    stop_flag: Arc<Mutex<bool>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ProcessCapture {
    /// 为目标进程创建捕获，返回 (会话, 帧队列消费者)
    pub fn new(pid: u32) -> windows::core::Result<(Self, rtrb::Consumer<f32>)> {
        unsafe {
            let client = activate_process_loopback(pid)?;
            let mix_fmt_ptr = client.GetMixFormat()?;
            let fmt = &*mix_fmt_ptr;
            let (rate, channels, is_float) = wave_format_info(fmt);
            if !is_float {
                return Err(windows::core::Error::from(windows::core::HRESULT(0x88890008u32 as i32)));
            }

            // 20ms 缓冲（100ns 单位）
            let buffer_duration: i64 = 200_000;
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                buffer_duration,
                0,
                fmt,
                None,
            )?;

            let event = windows::Win32::System::Threading::CreateEventW(None, false, false, None)?;
            client.SetEventHandle(event)?;
            let event_s = SendPtr(event);
            let capture: IAudioCaptureClient = client.GetService()?;

            client.Start()?;

            // 100ms 容量环形队列（技术设计§7）
            let (mut producer, consumer) =
                rtrb::RingBuffer::<f32>::new(rate as usize * channels as usize / 10);
            let stop_flag = Arc::new(Mutex::new(false));
            let stop_clone = stop_flag.clone();

            let fmt_box = Box::from_raw(mix_fmt_ptr);
            let capture_s = SendPtr(capture);
            let client_s = SendPtr(client);
            let thread = std::thread::Builder::new()
                .name(format!("capture-{pid}"))
                .spawn(move || {
                    capture_thread(stop_clone, event_s, capture_s, client_s, channels, producer, fmt_box);
                })?;

            Ok((
                Self {
                    pid,
                    sample_rate: rate,
                    channels,
                    stop_flag,
                    thread: Some(thread),
                },
                consumer,
            ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_format_info_pcm() {
        let mut fmt: WAVEFORMATEX = unsafe { std::mem::zeroed() };
        fmt.wFormatTag = 1u16; /* WAVE_FORMAT_PCM */
        fmt.nSamplesPerSec = 48000;
        fmt.nChannels = 2;
        let (rate, ch, is_float) = wave_format_info(&fmt);
        assert_eq!((rate, ch, is_float), (48000, 2, false));
    }

    #[test]
    #[ignore = "需要真实进程在播放音频，集成验证时手动执行"]
    fn capture_real_process() {
        // TC-001 集成自测占位：对指定 PID 捕获并断言 RMS>0
    }
}


/// 捕获线程体（独立函数便于编译器精确定位）
fn capture_thread(
    stop: std::sync::Arc<std::sync::Mutex<bool>>,
    event: SendPtr<windows::Win32::Foundation::HANDLE>,
    capture: SendPtr<IAudioCaptureClient>,
    client: SendPtr<IAudioClient>,
    channels: u16,
    mut producer: rtrb::Producer<f32>,
    fmt_box: Box<WAVEFORMATEX>,
) {
    unsafe {
        loop {
            if *stop.lock().unwrap() {
                break;
            }
            if WaitForSingleObject(event.0, 2000)
                != windows::Win32::Foundation::WAIT_OBJECT_0
            {
                continue;
            }
            let mut packet_size = match capture.0.GetNextPacketSize() {
                Ok(n) => n,
                Err(_) => break,
            };
            while packet_size > 0 {
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut nframes = 0u32;
                let mut flags: u32 = 0;
                if capture
                    .0
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
                let _ = capture.0.ReleaseBuffer(nframes);
                match capture.0.GetNextPacketSize() {
                    Ok(n) => packet_size = n,
                    Err(_) => break,
                }
            }
        }
        let _ = client.0.Stop();
    }
    drop(fmt_box);
}
