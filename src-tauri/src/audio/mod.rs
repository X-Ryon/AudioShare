pub mod mixer;
pub mod device_enum;
pub mod capture;
pub mod mic_capture;
pub mod virtual_dev;

use windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE;

/// 管线统一格式：48kHz / 2ch / 32bit float（WAVEFORMATEXTENSIBLE + IEEE_FLOAT）。
/// 麦克风与虚拟声卡均请求此格式，共享模式下由音频引擎自动完成重采样与声道转换，
/// 保证与进程环回（固定 48k/2ch）一致，消除采样率/位深不匹配导致的失真。
pub(crate) fn fixed_float_format() -> WAVEFORMATEXTENSIBLE {
    let mut ext: WAVEFORMATEXTENSIBLE = unsafe { std::mem::zeroed() };
    ext.Format.wFormatTag = 0xFFFE; // WAVE_FORMAT_EXTENSIBLE
    ext.Format.nChannels = 2;
    ext.Format.nSamplesPerSec = 48000;
    ext.Format.wBitsPerSample = 32;
    ext.Format.nBlockAlign = 8; // 2ch * 32bit / 8
    ext.Format.nAvgBytesPerSec = 48000 * 8;
    ext.Format.cbSize = 22; // sizeof(WAVEFORMATEXTENSIBLE) - sizeof(WAVEFORMATEX)
    ext.Samples.wValidBitsPerSample = 32;
    ext.dwChannelMask = 0x3; // SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT
    ext.SubFormat = windows::core::GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71); // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
    ext
}
