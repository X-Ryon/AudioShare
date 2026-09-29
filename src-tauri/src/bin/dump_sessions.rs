//! 临时诊断脚本：枚举所有音频会话，打印可获取的全部信息
//! 运行：cargo run --bin dump_sessions --release

use windows::core::{Interface, PWSTR};
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2, ISimpleAudioVolume,
    IMMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};

fn pwstr_to_string(ptr: PWSTR) -> String {
    unsafe {
        if ptr.0.is_null() {
            return String::new();
        }
        let mut len = 0usize;
        while *ptr.0.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr.0, len))
    }
}

fn pcwstr_to_string(ptr: windows::core::PCWSTR) -> String {
    unsafe {
        if ptr.0.is_null() {
            return String::new();
        }
        let mut len = 0usize;
        while *ptr.0.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr.0, len))
    }
}

fn process_name(pid: u32) -> String {
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return format!("<OpenProcess failed for pid {pid}>");
        };
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
            String::from_utf16_lossy(&buf[..len as usize])
        } else {
            format!("<QueryFullProcessImageNameW failed for pid {pid}>")
        }
    }
}

fn session_state_str(state: i32) -> &'static str {
    match state {
        0 => "Inactive",
        1 => "Active",
        2 => "Expired",
        _ => "Unknown",
    }
}

fn main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator = CoCreateInstance(
            &windows::Win32::Media::Audio::MMDeviceEnumerator,
            None,
            CLSCTX_ALL,
        )
        .expect("Failed to create MMDeviceEnumerator");

        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .expect("Failed to get default render endpoint");

        let device_name = device
            .GetId()
            .map(|id| pwstr_to_string(id))
            .unwrap_or_else(|_| "<unknown>".into());
        println!("=== 默认渲染设备: {device_name} ===\n");

        let manager: IAudioSessionManager2 = device
            .Activate(CLSCTX_ALL, None)
            .expect("Failed to activate IAudioSessionManager2");

        let sessions = manager
            .GetSessionEnumerator()
            .expect("Failed to get session enumerator");

        let count = sessions.GetCount().unwrap_or(0);
        println!("总会话数: {count}\n");
        println!("{}", "=".repeat(80));

        for i in 0..count {
            let Ok(session) = sessions.GetSession(i) else {
                println!("[{i}] <GetSession failed>");
                continue;
            };
            let Ok(ctrl2) = session.cast::<IAudioSessionControl2>() else {
                println!("[{i}] <cast IAudioSessionControl2 failed>");
                continue;
            };

            println!("\n--- 会话 #{i} ---");

            // 1. PID
            let pid = ctrl2.GetProcessId().unwrap_or(0);
            println!("  PID:                  {pid}");

            // 2. 完整进程路径
            let full_path = process_name(pid);
            println!("  完整进程路径:         {full_path}");

            // 3. 进程文件名
            let exe_name = full_path.rsplit('\\').next().unwrap_or(&full_path);
            println!("  可执行文件名 (exe):   {exe_name}");

            // 4. Display Name
            match ctrl2.GetDisplayName() {
                Ok(name) => println!("  会话显示名:           {}", pwstr_to_string(name)),
                Err(e) => println!("  会话显示名:           <error: {e}>"),
            }

            // 5. Icon Path
            match ctrl2.GetIconPath() {
                Ok(icon) => println!("  图标资源路径:         {}", pwstr_to_string(icon)),
                Err(e) => println!("  图标资源路径:         <error: {e}>"),
            }

            // 6. Session Identifier
            match ctrl2.GetSessionIdentifier() {
                Ok(id) => println!("  会话标识 (GUID):      {}", pwstr_to_string(id)),
                Err(e) => println!("  会话标识 (GUID):      <error: {e}>"),
            }

            // 7. Session Instance Identifier
            match ctrl2.GetSessionInstanceIdentifier() {
                Ok(id) => println!("  会话实例标识 (GUID):  {}", pwstr_to_string(id)),
                Err(e) => println!("  会话实例标识 (GUID):  <error: {e}>"),
            }

            // 8. Grouping Param
            match ctrl2.GetGroupingParam() {
                Ok(guid) => println!("  分组参数 (GUID):      {{{guid:?}}}"),
                Err(e) => println!("  分组参数 (GUID):      <error: {e}>"),
            }

            // 9. State
            match ctrl2.GetState() {
                Ok(state) => println!("  会话状态:             {} (raw={})", session_state_str(state.0), state.0),
                Err(e) => println!("  会话状态:             <error: {e}>"),
            }

            // 10. IsSystemSoundsSession
            let is_sys = ctrl2.IsSystemSoundsSession();
            println!("  系统声音会话:         {} (HRESULT={:#x})", if is_sys.0 == 0 { "是" } else { "否" }, is_sys.0);

            // 11. Peak Value (IAudioMeterInformation)
            match session.cast::<IAudioMeterInformation>() {
                Ok(meter) => match meter.GetPeakValue() {
                    Ok(peak) => println!("  实时峰值电平:         {:.6} ({})", peak, if peak > 0.0001 { "播放中" } else { "静音" }),
                    Err(e) => println!("  实时峰值电平:         <error: {e}>"),
                },
                Err(e) => println!("  实时峰值电平:         <cast failed: {e}>"),
            }

            // 12. Volume & Mute (ISimpleAudioVolume)
            match session.cast::<ISimpleAudioVolume>() {
                Ok(vol) => {
                    match vol.GetMasterVolume() {
                        Ok(v) => println!("  系统混音器音量:       {:.2} ({}%)", v, (v * 100.0) as u8),
                        Err(e) => println!("  系统混音器音量:       <error: {e}>"),
                    }
                    match vol.GetMute() {
                        Ok(m) => println!("  是否静音:             {}", if m.as_bool() { "是" } else { "否" }),
                        Err(e) => println!("  是否静音:             <error: {e}>"),
                    }
                }
                Err(e) => println!("  ISimpleAudioVolume:   <cast failed: {e}>"),
            }
        }

        println!("\n{}\n=== 枚举完成 ===", "=".repeat(80));
    }
}
