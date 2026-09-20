// 配置模块：解析 exe 同目录下的 config.txt
// 支持 UTF-8 / UTF-16LE(记事本另存) / 宽松键名 / 引号包裹
use crate::logger;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub portal: String,
    pub username: String,
    pub password: String,
    pub ac_name: String,
    pub auto_login: bool,
    pub relogin_interval: u64,
    pub probe_urls: Vec<String>,
}

pub const DEFAULT_CONFIG: &str = r#"# ==================================================
#  校园网自动登录 配置文件
#  请先填写 portal / username / password 三项
#  改完保存后，右键托盘图标 -> 一键登录 即可生效
# ==================================================

# 认证服务器地址 (写 主机:端口，端口不写默认 801；也可写完整 URL)
portal=

# 账号 (如运营商要求后缀，请直接带上，例如 学号@cmcc)
username=

# 密码
password=

# AC 名称 (一般保持默认即可，留空则使用 NAS)
ac_name=NAS

# 开机后自动登录: 1=开启 0=关闭
auto_login=1

# 自动检测间隔(秒，默认 1800 即 30 分钟，最小 30)
relogin_interval=1800
"#;

impl Config {
    pub fn path() -> PathBuf {
        logger::exe_dir().join("config.txt")
    }

    /// 读取配置；文件不存在时生成模板并返回 Err 提示
    pub fn load_or_create() -> Result<Config, String> {
        let path = Self::path();
        if !path.exists() {
            if let Err(e) = std::fs::write(&path, DEFAULT_CONFIG) {
                return Err(format!("生成配置模板失败({}): {}", path.display(), e));
            }
            return Err(format!(
                "未找到配置文件，已在 {} 生成模板，请编辑账号密码后重试",
                path.display()
            ));
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("读取配置失败: {}", e))?;
        let text = decode_text(&bytes);
        let mut map: Vec<(String, String)> = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(pos) = line.find('=') {
                let key = line[..pos].trim().to_lowercase();
                let mut val = line[pos + 1..].trim().to_string();
                if val.len() >= 2
                    && ((val.starts_with('"') && val.ends_with('"'))
                        || (val.starts_with('\'') && val.ends_with('\'')))
                {
                    val = val[1..val.len() - 1].to_string();
                }
                if !key.is_empty() {
                    map.push((key, val));
                }
            }
        }
        let get = |keys: &[&str]| -> Option<String> {
            for k in keys {
                for (mk, mv) in &map {
                    if mk == k {
                        return Some(mv.clone());
                    }
                }
            }
            None
        };

        let mut username = get(&["username", "user", "user_account", "account", "账号", "用户名"])
            .unwrap_or_default();
        let password =
            get(&["password", "passwd", "pwd", "密码"]).unwrap_or_default();
        let portal_raw =
            get(&["portal", "host", "server", "url", "服务器", "地址"]).unwrap_or_default();
        let ac_name = get(&["ac_name", "acname", "wlan_ac_name"]).unwrap_or_default();
        let auto_login = get(&["auto_login", "auto", "自动登录"])
            .map(|v| !matches!(v.trim(), "0" | "false" | "off" | "no"))
            .unwrap_or(true);
        let relogin_interval = get(&["relogin_interval", "interval", "间隔"])
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(1800)
            .clamp(30, 86400);
        let probe_urls = get(&["probe_urls", "probe", "检测地址"])
            .map(|v| {
                v.split(['|', ',', ';'])
                    .map(|s| s.trim().to_string())
                    .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let portal = normalize_portal(&portal_raw);
        if portal.is_empty() {
            return Err(format!("请在 {} 中填写 portal(认证服务器地址)", path.display()));
        }
        username = username.trim().to_string();

        // 兼容 isp 后缀写法：username=学号 + isp=cmcc
        if !username.contains('@') {
            if let Some(isp) = get(&["isp", "运营商"]) {
                let isp = isp.trim().trim_start_matches('@').to_string();
                if !isp.is_empty() {
                    username = format!("{}@{}", username, isp);
                }
            }
        }

        if username.is_empty() || password.is_empty() {
            return Err(format!(
                "请在 {} 中填写 username 与 password",
                path.display()
            ));
        }

        Ok(Config {
            portal,
            username,
            password,
            ac_name: if ac_name.is_empty() { "NAS".into() } else { ac_name },
            auto_login,
            relogin_interval,
            probe_urls: if probe_urls.is_empty() {
                vec!["http://www.baidu.com/".to_string()]
            } else {
                probe_urls
            },
        })
    }
}

/// UTF-8 / UTF-16LE / UTF-16BE BOM 识别，否则按 UTF-8 宽松解码
fn decode_text(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let body = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    };
    String::from_utf8_lossy(body).into_owned()
}

/// 规整为 host:port 形式；端口缺省 801
pub fn normalize_portal(s: &str) -> String {
    let mut s = s.trim().to_string();
    if let Some(i) = s.find("://") {
        s = s[i + 3..].to_string();
    }
    if let Some(i) = s.find('/') {
        s.truncate(i);
    }
    s = s.trim().trim_end_matches('/').to_string();
    if s.is_empty() {
        return s;
    }
    if s.contains(':') {
        s
    } else {
        format!("{}:801", s)
    }
}
