//! 虚拟声卡写出（技术设计§4 virtual_dev.rs）
//! 渲染到 VB-CABLE "CABLE Input" 端点，系统即呈现为虚拟麦克风。
//! 追溯：TC-006（虚拟麦就绪）、TC-011（未装引导安装）

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    IAudioClient, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, eRender,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::STGM_READ;
use windows::Win32::System::Threading::WaitForSingleObject;

use super::device_enum::CABLE_INPUT_NAME;

fn find_cable_endpoint(enumerator: &IMMDeviceEnumerator) -> windows::core::Result<IMMDevice> {
    unsafe {
        let collection = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for i in 0..collection.GetCount()? {
            let dev = collection.Item(i)?;
            // STGM_READ（=0）；误用 STGM_WRITE(1) 会导致 GetValue 返回 E_ACCESSDENIED
            let store = dev.OpenPropertyStore(STGM_READ)?;
            let pk = PROPERTYKEY {
                fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
                pid: 14,
            };
            let prop = store.GetValue(&pk)?;
            if prop.Anonymous.Anonymous.vt.0 == 31 {
                let pwsz = prop.Anonymous.Anonymous.Anonymous.pwszVal;
                let mut len = 0usize;
                while *pwsz.0.add(len) != 0 {
                    len += 1;
                }
                let name = String::from_utf16_lossy(std::slice::from_raw_parts(pwsz.0, len));
                if name.contains(CABLE_INPUT_NAME) {
                    return Ok(dev);
                }
            }
        }
        Err(windows::core::Error::from(windows::core::HRESULT(0x80070490u32 as i32))) // 元素未找到
    }
}

/// 虚拟麦写出端点
pub struct VirtualSink {
    pub sample_rate: u32,
    pub channels: u16,
    client: IAudioClient,
    render: IAudioRenderClient,
    event: windows::Win32::Foundation::HANDLE,
    buffer_frames: u32,
}

impl VirtualSink {
    pub fn new() -> windows::core::Result<Self> {
        super::device_enum::ensure_com();
        unsafe {
            let enumerator = super::device_enum::new_device_enumerator()?;
            let dev = find_cable_endpoint(&enumerator)?;
            let client: IAudioClient = dev.Activate(windows::Win32::System::Com::CLSCTX_ALL, None)?;
            // 请求固定 48kHz/2ch/f32 格式（共享模式引擎自动转换），与管线一致
            let ext = super::fixed_float_format();
            let fmt = &ext.Format;
            let rate = fmt.nSamplesPerSec;
            let channels = fmt.nChannels;
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                200_000,
                0,
                fmt,
                None,
            )?;
            let event =
                windows::Win32::System::Threading::CreateEventW(None, false, false, None)?;
            client.SetEventHandle(event)?;
            let render: IAudioRenderClient = client.GetService()?;
            let buffer_frames = client.GetBufferSize()?;
            client.Start()?;
            // 先填充静音，防止启动初期的旧数据
            let data = render.GetBuffer(buffer_frames)?;
            std::ptr::write_bytes(data, 0, buffer_frames as usize * channels as usize * 4);
            render.ReleaseBuffer(buffer_frames, 0)?;
            Ok(Self {
                sample_rate: rate,
                channels,
                client,
                render,
                event,
                buffer_frames,
            })
        }
    }

    /// 等待写出事件（超时返回 false）
    pub fn wait(&self, timeout_ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.event, timeout_ms) == windows::Win32::Foundation::WAIT_OBJECT_0 }
    }

    /// 写入一帧混音结果（interleaved f32，声道数须匹配）
    pub fn write(&self, frames: &[f32]) -> windows::core::Result<()> {
        unsafe {
            let padding = self.client.GetCurrentPadding()?;
            let available = self.buffer_frames - padding;
            let n = (frames.len() / self.channels as usize).min(available as usize);
            if n == 0 {
                return Ok(());
            }
            let data = self.render.GetBuffer(n as u32)?;
            std::ptr::copy_nonoverlapping(
                frames.as_ptr(),
                data as *mut f32,
                n * self.channels as usize,
            );
            self.render.ReleaseBuffer(n as u32, 0)?;
            Ok(())
        }
    }
}
