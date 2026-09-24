//! 物理麦克风采集（技术设计§4 mic_capture.rs）
//! 追溯：TC-008（麦克风切换实时生效、不断流）

use windows::core::{Interface, PCWSTR};
use windows::Win32::Media::Audio::{
    IAudioClient, IAudioCaptureClient, IMMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_SHAREMODE_SHARED,
    eCapture,
};
use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};
use windows::Win32::Media::Audio::eConsole;

use super::capture::wave_format_info;

/// 跨线程传递 COM 接口的 Send 包装
struct SendPtr<T>(T);
unsafe impl<T> Send for SendPtr<T> {}

/// 麦克风采集会话：推 f32 帧入环形队列
pub struct MicCapture {
    pub sample_rate: u32,
    pub channels: u16,
    stop_flag: std::sync::Arc<std::sync::Mutex<bool>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MicCapture {
    /// device_id 为空串时使用系统默认设备
    pub fn new(device_id: &str) -> windows::core::Result<(Self, rtrb::Consumer<f32>)> {
        unsafe {
            let enumerator: IMMDeviceEnumerator = super::device_enum::new_device_enumerator()?;
            let dev = if device_id.is_empty() {
                enumerator.GetDefaultAudioEndpoint(eCapture, eConsole)?
            } else {
                let mut buf: Vec<u16> = device_id.encode_utf16().collect();
                buf.push(0);
                enumerator.GetDevice(PCWSTR(buf.as_ptr()))?
            };

            let client: IAudioClient = dev.Activate(windows::Win32::System::Com::CLSCTX_ALL, None)?;
            let mix_fmt_ptr = client.GetMixFormat()?;
            let fmt = &*mix_fmt_ptr;
            let (rate, channels, is_float) = wave_format_info(fmt);
            if !is_float {
                return Err(windows::core::Error::from(windows::core::HRESULT(0x88890008u32 as i32)));
            }

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

            let (mut producer, consumer) =
                rtrb::RingBuffer::<f32>::new(rate as usize * channels as usize / 10);
            let stop_flag = std::sync::Arc::new(std::sync::Mutex::new(false));
            let stop_clone = stop_flag.clone();
            let fmt_box = Box::from_raw(mix_fmt_ptr);
            let event_s = SendPtr(event);
            let capture_s = SendPtr(capture);
            let client_s = SendPtr(client);
            let thread = std::thread::Builder::new()
                .name("mic-capture".into())
                .spawn(move || {
                    mic_capture_thread(stop_clone, event_s, capture_s, client_s, channels, producer, fmt_box);
                })?;

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

/// 麦克风捕获线程体
fn mic_capture_thread(
    stop: std::sync::Arc<std::sync::Mutex<bool>>,
    event: SendPtr<windows::Win32::Foundation::HANDLE>,
    capture: SendPtr<IAudioCaptureClient>,
    client: SendPtr<IAudioClient>,
    channels: u16,
    mut producer: rtrb::Producer<f32>,
    _fmt_box: Box<windows::Win32::Media::Audio::WAVEFORMATEX>,
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
                if flags != windows::Win32::Media::Audio::AUDCLNT_BUFFERFLAGS_SILENT.0 as u32
                    && !data.is_null()
                {
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
}
