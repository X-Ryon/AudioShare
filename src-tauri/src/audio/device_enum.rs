//! 设备与应用枚举（技术设计§4 device_enum.rs）
//! 追溯：TC-008（麦克风枚举）、TC-016（仅列具备扬声器权限的应用）、TC-006/011（CABLE 检测）

use serde::Serialize;
use windows::core::PCWSTR;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eRender, IMMDevice, IMMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::STGM;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::ProcessStatus::K32EnumProcesses;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::Interface;

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

/// 创建设备枚举器（MMDeviceEnumerator CLSID）
pub(crate) fn new_device_enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
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
        let store = dev.OpenPropertyStore(STGM(1) /* STGM_READ */)?;
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

/// 枚举所有进程，返回候选音频应用列表（过滤系统进程与 UWP）。
/// 注：WASAPI Process Loopback 按 PID 激活，启动播放后即可捕获；
/// 这里按可执行名聚合，过滤掉明显的系统/后台进程。
/// 追溯 TC-016。
pub fn list_audio_apps() -> Vec<AudioApp> {
    unsafe {
        let mut pids = [0u32; 2048];
        let mut needed = 0u32;
        if !K32EnumProcesses(pids.as_mut_ptr(), pids.len() as u32, &mut needed).as_bool() {
            return Vec::new();
        }
        let count = needed as usize / std::mem::size_of::<u32>();

        // 过滤名单：系统进程与无 UI 的后台进程
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
        ];

        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for &pid in &pids[..count] {
            if pid == 0 {
                continue;
            }
            let Some(exe) = process_name(pid) else { continue };
            let lower = exe.to_lowercase();
            // UWP 包进程以 ApplicationFrameHost 之外难以直接 loopback，过滤已知 UWP 形态
            if lower.starts_with("application") {
                continue;
            }
            if SYSTEM_EXES.iter().any(|s| lower == *s) {
                continue;
            }
            // 同名进程只展示一次（按可执行名聚合，Chrome 多进程归并）
            if !seen.insert(lower.clone()) {
                continue;
            }
            out.push(AudioApp {
                pid,
                exe,
                name: exe_display_name(&lower),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
