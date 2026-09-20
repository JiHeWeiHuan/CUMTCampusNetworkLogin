// 轻量日志模块：写入 exe 同目录（不可写时退回临时目录），超限自动轮转
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_SIZE: u64 = 512 * 1024;
static LOG_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// exe 所在目录（不可得时退回临时目录）
pub fn exe_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            return parent.to_path_buf();
        }
    }
    std::env::temp_dir()
}

pub fn init() {
    let dir = exe_dir();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("campus-login.log");
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > MAX_LOG_SIZE {
            let _ = fs::remove_file(&path);
        }
    }
    if let Ok(mut guard) = LOG_PATH.lock() {
        *guard = Some(path);
    }
}

pub fn write(level: &str, msg: &str) {
    let line = format!("[{}] [{}] {}\r\n", fmt_now(), level, msg);
    if let Ok(guard) = LOG_PATH.lock() {
        if let Some(path) = guard.as_ref() {
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = f.write_all(line.as_bytes());
            }
        }
    }
}

/// 本地时间(UTC+8)格式化，不引入外部依赖
fn fmt_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + 8 * 3600;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, h, mi, s)
}

/// Howard Hinnant 的 days->civil 算法
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
