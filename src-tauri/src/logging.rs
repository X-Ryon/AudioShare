//! 日志体系：全应用异常与关键动作的唯一落盘出口
//!
//! 设计：
//! - 落盘：单文件 `%APPDATA%\AudioShare\logs\audioshare.log`，追加写；
//!   超过 4MB 轮转为 `audioshare.log.old`（仅保留一代历史，个人工具避免磁盘膨胀）
//! - 级别：ERROR=异常失败（WASAPI/COM 初始化、安装器、配置 IO、panic）；
//!   WARN=可恢复降级（麦克风打开失败、自动恢复共享失败、配置解析失败）；
//!   INFO=关键动作（启动、退出、引擎初始化）
//! - 格式：`[YYYY-MM-DD HH:MM:SS.mmm][LEVEL][target] message`；
//!   时间戳为系统本地时间（epoch 换算 + GetTimeZoneInformation 偏移，不引入第三方时间 crate）
//! - panic hook：任意线程 panic 先写 ERROR 日志并 flush 再走默认崩溃流程，保证现场可查
//! - 前端通道：`write_log` 命令（target=frontend），Vue 侧异常同样落盘
//! - 诊断二进制（src/bin/*）为临时工具，不走本体系，保留 println!/eprintln!

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

use log::{Level, LevelFilter, Metadata, Record};
use windows::Win32::System::SystemServices::TIME_ZONE_ID_DAYLIGHT;
use windows::Win32::System::Time::GetTimeZoneInformation;

/// 单文件上限：超出即轮转为 .old
const MAX_BYTES: u64 = 4 * 1024 * 1024;

static LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

struct FileLogger;

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "[{}][{:>5}][{}] {}\n",
            timestamp(),
            record.level(),
            record.target(),
            record.args()
        );
        if let Ok(mut guard) = LOG_FILE.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.write_all(line.as_bytes());
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut guard) = LOG_FILE.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

static LOGGER: FileLogger = FileLogger;

/// 初始化日志：建目录、超限轮转、挂载全局 logger 与 panic hook。
/// 失败返回 Err（调用方降级为 eprintln!，不影响主流程）。
pub fn init(config_dir: &std::path::Path) -> Result<(), String> {
    let dir = config_dir.join("logs");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建日志目录失败: {e}"))?;
    let path = dir.join("audioshare.log");
    // 体积轮转：超限将当前文件改名为 .old（覆盖更旧的 .old）
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > MAX_BYTES {
            let old = dir.join("audioshare.log.old");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&path, &old);
        }
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("打开日志文件失败: {e}"))?;
    *LOG_FILE.lock().unwrap() = Some(file);
    log::set_logger(&LOGGER).map_err(|e| format!("挂载 logger 失败: {e}"))?;
    log::set_max_level(LevelFilter::Info);

    // panic hook：崩溃现场先落盘（flush 确保写完成）
    std::panic::set_hook(Box::new(|info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".into());
        log::error!(target: "panic", "at {loc}: {info}");
        log::logger().flush();
    }));
    Ok(())
}

/// 本地时间时间戳：epoch 秒换算历法日期 + Windows 时区偏移（含夏令时）
fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let millis = now.subsec_millis();
    // 当前生效的时区偏移（分钟）：Bias 为 UTC→本地的负偏移，叠加标准/夏令时附加偏移
    let offset_min = unsafe {
        let mut tzi: windows::Win32::System::Time::TIME_ZONE_INFORMATION = std::mem::zeroed();
        let kind = GetTimeZoneInformation(&mut tzi);
        let extra = if kind == TIME_ZONE_ID_DAYLIGHT {
            tzi.DaylightBias
        } else {
            tzi.StandardBias
        };
        -(tzi.Bias as i64 + extra as i64)
    };
    let secs = now.as_secs() as i64 + offset_min * 60;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}.{millis:03}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// days_from_civil 逆算法（Howard Hinnant），epoch 天数 → (年, 月, 日)
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}
