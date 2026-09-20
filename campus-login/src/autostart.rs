// 开机自启：HKCU\Software\Microsoft\Windows\CurrentVersion\Run
#![cfg(windows)]

use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "CampusNetAutoLogin";

/// 幂等注册开机自启（路径变化时自动更新）
pub fn enable() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("获取程序路径失败: {}", e))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let path = format!("\"{}\"", exe.display());
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(RUN_KEY)
        .map_err(|e| format!("打开注册表失败: {}", e))?;
    let existing: Option<String> = key.get_value(VALUE_NAME).ok();
    if existing.as_deref() == Some(path.as_str()) {
        return Ok(());
    }
    key.set_value(VALUE_NAME, &path)
        .map_err(|e| format!("写入注册表失败: {}", e))?;
    Ok(())
}
