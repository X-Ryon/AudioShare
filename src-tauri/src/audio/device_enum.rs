//! 设备与应用枚举（技术设计§4 device_enum.rs）
//! 追溯：TC-008（麦克风枚举）、TC-016（仅列具备扬声器权限的应用）、TC-006/011（CABLE 检测）

use serde::Serialize;
use windows::core::{HRESULT, Interface, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2,
    IMMDevice, IMMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{CLSCTX_ALL, STGM_READ};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

#[derive(Debug, Clone, Serialize)]
pub struct MicDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioApp {
    pub pid: u32,
    pub exe: String,
    pub name: String,
    /// 当前是否正在播放（会话峰值电平 > 阈值）
    pub playing: bool,
}

/// VB-CABLE 的渲染端点设备名（写入端）
pub const CABLE_INPUT_NAME: &str = "CABLE Input";

fn pwstr_to_string(ptr: PCWSTR) -> String {
    unsafe {
        if ptr.is_null() {
            return String::new();
        }
        let mut len = 0usize;
        while *ptr.0.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr.0, len);
        String::from_utf16_lossy(slice)
    }
}

/// 确保当前线程已初始化 COM（WASAPI 必需；幂等）
pub(crate) fn ensure_com() {
    unsafe {
        // COINIT_MULTITHREADED；已初始化(含其他模式)时忽略 S_FALSE/RPC_E_CHANGED_MODE
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        );
    }
}

/// 创建设备枚举器（MMDeviceEnumerator CLSID）
pub(crate) fn new_device_enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    ensure_com();
    unsafe {
        let clsid = windows::core::GUID::from_u128(0xbcde0395_e52f_467c_8e3d_c4579291692e);
        windows::Win32::System::Com::CoCreateInstance(
            &clsid,
            None,
            windows::Win32::System::Com::CLSCTX_ALL,
        )
    }
}

fn get_friendly_name(dev: &IMMDevice) -> windows::core::Result<String> {
    unsafe {
        // STGM_READ（=0）；误用 STGM_WRITE(1) 会导致 GetValue 返回 E_ACCESSDENIED
        let store = dev.OpenPropertyStore(STGM_READ)?;
        let pk = PROPERTYKEY {
            fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
            pid: 14, // PKEY_Device_FriendlyName
        };
        let prop = store.GetValue(&pk)?;
        // PROVARIANT 联合体第一个字段是 VT，字符串时 Anonymous.Anonymous.pwszVal
        let vt = prop.Anonymous.Anonymous.vt;
        if vt.0 == 31 {
            // VT_LPWSTR
            let pwsz = prop.Anonymous.Anonymous.Anonymous.pwszVal;
            Ok(pwstr_to_string(PCWSTR(pwsz.0)))
        } else {
            Ok(String::new())
        }
    }
}

/// 枚举物理麦克风（采集设备），并标记系统默认。
/// 追溯 TC-008。
pub fn list_capture_devices() -> Vec<MicDevice> {
    ensure_com();
    unsafe {
        let Ok(enumerator) = new_device_enumerator() else {
            return Vec::new();
        };
        // 默认通信/控制台设备 id
        let default_id = enumerator
            .GetDefaultAudioEndpoint(eCapture, eCommunications)
            .ok()
            .and_then(|d| d.GetId().ok())
            .map(|id| pwstr_to_string(PCWSTR(id.0)))
            .unwrap_or_default();

        let mut out = Vec::new();
        if let Ok(collection) = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) {
            for i in 0..collection.GetCount().unwrap_or(0) {
                if let Ok(dev) = collection.Item(i) {
                    if let (Ok(id), Ok(name)) = (dev.GetId(), get_friendly_name(&dev)) {
                        // 过滤 VB-CABLE 虚拟设备：其输出端被选为麦克风输入时，
                        // 会与写入 "CABLE Input" 的混音形成回路自激（失真），直接从列表剔除
                        if name.to_uppercase().contains("CABLE") {
                            continue;
                        }
                        let id_s = pwstr_to_string(PCWSTR(id.0));
                        out.push(MicDevice {
                            is_default: id_s == default_id,
                            id: id_s,
                            name,
                        });
                    }
                }
            }
        }
        out
    }
}

/// 检测 VB-CABLE 虚拟声卡是否已安装（检索渲染端点是否存在 "CABLE Input"）。
/// 追溯 TC-006 / TC-011。
pub fn detect_virtual_cable() -> bool {
    ensure_com();
    unsafe {
        let Ok(enumerator) = new_device_enumerator() else {
            return false;
        };
        let Ok(collection) = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else {
            return false;
        };
        for i in 0..collection.GetCount().unwrap_or(0) {
            if let Ok(dev) = collection.Item(i) {
                if let Ok(name) = get_friendly_name(&dev) {
                    if name.contains(CABLE_INPUT_NAME) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// 检测系统默认输入设备（控制台角色）是否为 VB-CABLE。
/// 引导第 2 步轮询用：默认麦克风必须设为 CABLE Input，会议软件才能收到共享音频。
pub fn default_mic_is_cable() -> bool {
    ensure_com();
    unsafe {
        let Ok(enumerator) = new_device_enumerator() else {
            return false;
        };
        let Ok(dev) = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole) else {
            return false; // 无任何采集设备
        };
        let Ok(name) = get_friendly_name(&dev) else {
            return false;
        };
        name.to_uppercase().contains("CABLE")
    }
}

fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        if QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok()
        {
            let full = String::from_utf16_lossy(&buf[..len as usize]);
            Some(full.rsplit('\\').next().unwrap_or(&full).to_string())
        } else {
            None
        }
    }
}

fn exe_display_name(exe: &str) -> String {
    let lower = exe.to_lowercase();
    let known = [
        ("cloudmusic.exe", "网易云音乐"),
        ("douyin.exe", "抖音"),
        ("qqmusic.exe", "QQ音乐"),
        ("kugou.exe", "酷狗音乐"),
        ("kwplayer.exe", "酷我音乐"),
        ("chrome.exe", "Chrome"),
        ("msedge.exe", "Edge"),
        ("firefox.exe", "Firefox"),
        ("steam.exe", "Steam"),
        ("wechat.exe", "微信"),
        ("qq.exe", "QQ"),
        ("bilibili.exe", "哔哩哔哩"),
        ("potplayermini64.exe", "PotPlayer"),
        ("vlc.exe", "VLC"),
    ];
    for (k, v) in known {
        if lower == k {
            return v.to_string();
        }
    }
    exe.trim_end_matches(".exe").to_string()
}

/// 枚举音频会话，返回具备音频能力（创建过会话）的应用列表。
/// 基于 IAudioSessionManager2 会话枚举（粒度1）：只有创建过音频会话的进程才出现，
/// 天然排除无音频能力的进程；通过 IAudioMeterInformation::GetPeakValue 判定"正在播放"。
/// 按 exe 归并（Chrome 多进程），playing 取任一会话。
/// 追溯 TC-016。
pub fn list_audio_apps() -> Vec<AudioApp> {
    ensure_com();
    unsafe {
        let Ok(enumerator) = new_device_enumerator() else {
            return Vec::new();
        };
        // 默认渲染端点（控制台角色），音频会话挂在渲染设备上
        let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, eConsole) else {
            return Vec::new();
        };
        let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
            return Vec::new();
        };
        let Ok(sessions) = manager.GetSessionEnumerator() else {
            return Vec::new();
        };

        // 过滤名单：系统进程、无 UI 的后台进程与本应用自身（写虚拟声卡的会话）
        const SYSTEM_EXES: &[&str] = &[
            "system", "registry", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe",
            "services.exe", "lsass.exe", "svchost.exe", "fontdrvhost.exe", "dwm.exe",
            "conhost.exe", "sihost.exe", "taskhostw.exe", "explorer.exe", "ctfmon.exe",
            "chredt.exe", "dllhost.exe", "runtimebroker.exe", "searchhost.exe",
            "startmenuexperiencehost.exe", "textinputhost.exe", "shellexperiencehost.exe",
            "applicationframehost.exe", "systemsettings.exe", "securityhealthservice.exe",
            "securityhealthsystray.exe", "spoolsv.exe", "audiodg.exe", "wudfhost.exe",
            "msmpeng.exe", "nissrv.exe", "wmiadap.exe", "tiworker.exe", "vmmem.exe",
            "python.exe", "pythonw.exe", "node.exe", "cargo.exe", "rustc.exe", "link.exe",
            "mspdbsrv.exe", "powershell.exe", "pwsh.exe", "cmd.exe", "bash.exe",
            "git.exe", "qwenwork.exe", "code.exe", "cursor.exe", "devenv.exe",
            "audioshare.exe",
        ];

        // exe(小写) -> 聚合结果（同名多进程归并，playing 取 OR）
        let mut seen: std::collections::HashMap<String, AudioApp> =
            std::collections::HashMap::new();
        for i in 0..sessions.GetCount().unwrap_or(0) {
            let Ok(session) = sessions.GetSession(i) else { continue };
            let Ok(ctrl2) = session.cast::<IAudioSessionControl2>() else { continue };
            // 跳过系统声音会话（System Sounds）：S_OK 表示是，S_FALSE 表示不是
            if ctrl2.IsSystemSoundsSession().0 == 0 {
                continue;
            }
            let Ok(pid) = ctrl2.GetProcessId() else { continue };
            if pid == 0 {
                continue;
            }
            let Some(exe) = process_name(pid) else { continue };
            let lower = exe.to_lowercase();
            if lower.starts_with("application") {
                continue;
            }
            if SYSTEM_EXES.iter().any(|s| lower == *s) {
                continue;
            }
            // 会话峰值电平：>0.0001 视为正在播放（每次枚举新建 meter，取近 3 秒峰值）
            let playing = session
                .cast::<IAudioMeterInformation>()
                .ok()
                .and_then(|m| m.GetPeakValue().ok())
                .map(|p| p > 0.0001)
                .unwrap_or(false);
            match seen.get_mut(&lower) {
                Some(app) => app.playing |= playing,
                None => {
                    seen.insert(
                        lower.clone(),
                        AudioApp {
                            pid,
                            exe,
                            name: exe_display_name(&lower),
                            playing,
                        },
                    );
                }
            }
        }
        let mut out: Vec<AudioApp> = seen.into_values().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

/// 启动切换判定（纯函数，TDD）：CABLE 存在且默认非 CABLE 才切
/// 追溯 TC-031/033/034。
pub fn should_switch_mic(default_is_cable: bool, cable_exists: bool) -> bool {
    cable_exists && !default_is_cable
}

/// 退出回切判定（纯函数，TDD）：有原设备记录且 CABLE 存在才回切
/// 追溯 TC-032/034。
pub fn should_restore_mic(saved: Option<&str>, cable_exists: bool) -> bool {
    saved.is_some() && cable_exists
}

/// IPolicyConfig（手写 COM）：vtable 槽位 0-9 为占位方法（仅保证 ABI 对齐，运行时不调用），
/// 槽位 10=SetDefaultEndpoint（唯一调用点）、槽位 11=SetEndpointVisibility。
/// 占位参数统一用裸指针，避免为未用方法引入 WAVEFORMATEX/PROPVARIANT 等类型依赖。
#[windows::core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: windows::core::IUnknown {
    unsafe fn GetMixFormat(&self, pdevice: PCWSTR, pwfx: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetDeviceFormat(&self, pdevice: PCWSTR, bdefault: i32, pwfx: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn ResetDeviceFormat(&self, pdevice: PCWSTR) -> HRESULT;
    unsafe fn SetDeviceFormat(&self, pdevice: PCWSTR, pendpointformat: *mut core::ffi::c_void, pmixformat: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetProcessingPeriod(&self, pdevice: PCWSTR, bdefault: i32, phnperiod: *mut i64, phnminperiod: *mut i64) -> HRESULT;
    unsafe fn SetProcessingPeriod(&self, pdevice: PCWSTR, hnperiod: *const i64) -> HRESULT;
    unsafe fn GetShareMode(&self, pdevice: PCWSTR, pmode: *mut i32) -> HRESULT;
    unsafe fn SetShareMode(&self, pdevice: PCWSTR, mode: i32) -> HRESULT;
    unsafe fn GetPropertyValue(&self, pdevice: PCWSTR, key: *mut core::ffi::c_void, pvalue: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn SetPropertyValue(&self, pdevice: PCWSTR, key: *mut core::ffi::c_void, pvalue: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn SetDefaultEndpoint(&self, pdevice: PCWSTR, erole: i32) -> HRESULT;
    unsafe fn SetEndpointVisibility(&self, pdevice: PCWSTR, bvisible: i32) -> HRESULT;
}

/// 查找 VB-Cable 采集端点 id：eCapture 枚举中友好名含 "CABLE" 的端点
/// （VB-Cable 虚拟麦克风在录音设备列表显示为 "CABLE Output"）。追溯 TC-031。
pub fn find_cable_capture_id() -> Option<String> {
    ensure_com();
    unsafe {
        let enumerator = new_device_enumerator().ok()?;
        let collection = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE).ok()?;
        for i in 0..collection.GetCount().unwrap_or(0) {
            if let Ok(dev) = collection.Item(i) {
                if let Ok(name) = get_friendly_name(&dev) {
                    if name.to_uppercase().contains("CABLE") {
                        if let Ok(id) = dev.GetId() {
                            return Some(pwstr_to_string(PCWSTR(id.0)));
                        }
                    }
                }
            }
        }
        None
    }
}

/// 当前默认输入设备（eConsole）id。追溯 TC-031/032。
pub fn get_default_capture_id() -> Option<String> {
    ensure_com();
    unsafe {
        let enumerator = new_device_enumerator().ok()?;
        let dev = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole).ok()?;
        let id = dev.GetId().ok()?;
        Some(pwstr_to_string(PCWSTR(id.0)))
    }
}

/// 设置默认输入设备：IPolicyConfig::SetDefaultEndpoint(id, eConsole + eCommunications)
/// 双角色都设置，保证会议/语音软件无论按哪个角色取默认都能拿到新设备。追溯 TC-031/032。
pub fn set_default_capture_device(id: &str) -> Result<(), String> {
    ensure_com();
    unsafe {
        // CLSID_IPolicyConfig {870af99c-171d-4f9e-af0d-e63df40c2bc9}（与 IID 相同）
        let clsid = windows::core::GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
        let pc: IPolicyConfig = windows::Win32::System::Com::CoCreateInstance(&clsid, None, CLSCTX_ALL)
            .map_err(|e| format!("创建 IPolicyConfig 失败: {e}"))?;
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        let p = PCWSTR(wide.as_ptr());
        pc.SetDefaultEndpoint(p, eConsole.0)
            .ok()
            .map_err(|e| format!("SetDefaultEndpoint(eConsole) 失败: {e}"))?;
        pc.SetDefaultEndpoint(p, eCommunications.0)
            .ok()
            .map_err(|e| format!("SetDefaultEndpoint(eCommunications) 失败: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TC-031/033/034：仅"默认非CABLE 且 CABLE 存在"才切换
    #[test]
    fn switch_only_when_cable_and_not_default() {
        assert!(should_switch_mic(false, true));
        assert!(!should_switch_mic(true, true));
        assert!(!should_switch_mic(false, false));
        assert!(!should_switch_mic(true, false));
    }

    /// TC-032/034：仅"有原设备记录 且 CABLE 存在"才回切
    #[test]
    fn restore_only_when_saved_and_cable() {
        assert!(should_restore_mic(Some("mic-id"), true));
        assert!(!should_restore_mic(Some("mic-id"), false));
        assert!(!should_restore_mic(None, true));
        assert!(!should_restore_mic(None, false));
    }

    /// 本机冒烟：查询函数调用不 panic（TC-031/032 真机验收，此处只验不崩溃）
    #[test]
    fn cable_and_default_id_queries_no_panic() {
        let _ = find_cable_capture_id();
        let _ = get_default_capture_id();
    }

    #[test]
    fn exe_display_name_known_apps() {
        assert_eq!(exe_display_name("cloudmusic.exe"), "网易云音乐");
        assert_eq!(exe_display_name("Douyin.EXE"), "抖音");
        assert_eq!(exe_display_name("chrome.exe"), "Chrome");
    }

    #[test]
    fn exe_display_name_unknown_falls_back() {
        assert_eq!(exe_display_name("SomeGame.exe"), "SomeGame");
    }

    #[test]
    fn list_capture_devices_returns_vec() {
        // 本机调用不 panic 即通过（内容因机器而异）
        let _ = list_capture_devices();
    }

    #[test]
    fn detect_virtual_cable_returns_bool() {
        let _ = detect_virtual_cable();
    }

    #[test]
    fn list_audio_apps_no_system_process() {
        let apps = list_audio_apps();
        assert!(apps.iter().all(|a| a.exe.to_lowercase() != "svchost.exe"));
        assert!(apps.iter().all(|a| a.exe.to_lowercase() != "audiodg.exe"));
        // 同名进程归并：exe 无重复
        let mut exes: Vec<String> = apps.iter().map(|a| a.exe.to_lowercase()).collect();
        exes.sort();
        exes.dedup();
        assert_eq!(exes.len(), apps.len());
    }
}
