// Dr.COM/eportal 认证协议
// 登录:  GET /eportal/?c=Portal&a=login&callback=dr{ts}&login_method=1&user_account=..&user_password=..
//        &wlan_user_ip=..&wlan_user_mac=..&wlan_ac_ip=&wlan_ac_name=..&jsVersion=3.0&_={ts}
// 响应:  JSONP `dr{ts}({"result":1,"msg":"..."})`，result=1 成功；ret_code=2 表示已在线
// 状态:  双探测 —— 门户可达性 + 外网可达性（跟随重定向关闭，识别强制跳转）
use crate::config::Config;
use crate::netinfo;
use serde_json::Value;
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const PORTAL_MARKERS: [&str; 4] = ["eportal", "wlan_user_ip", "wlan_user_mac", "c=Portal"];

pub fn build_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(4))
        .timeout(Duration::from_secs(10))
        .user_agent(UA)
        .build()
}

/// 关闭重定向的独立 agent，用于外网探测识别强制跳转
pub fn build_probe_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(4))
        .timeout(Duration::from_secs(8))
        .user_agent(UA)
        .build()
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 严格百分号编码（空格、@、中文等全部转义，避免密码含特殊字符时破坏 URL）
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

pub fn build_login_url(cfg: &Config, ip: &str, mac: &str) -> String {
    let ts = now_ms();
    format!(
        "http://{}/eportal/?c=Portal&a=login&callback=dr{}&login_method=1&user_account={}&user_password={}&wlan_user_ip={}&wlan_user_mac={}&wlan_ac_ip=&wlan_ac_name={}&jsVersion=3.0&_={}",
        cfg.portal,
        ts,
        enc(&cfg.username),
        enc(&cfg.password),
        enc(ip),
        enc(mac),
        enc(&cfg.ac_name),
        ts
    )
}

/// 剥掉 JSONP 包装，解析内部 JSON
pub fn parse_jsonp(body: &str) -> Option<Value> {
    let start = body.find('(')?;
    let end = body.rfind(')')?;
    if end <= start + 1 {
        return None;
    }
    serde_json::from_str(&body[start + 1..end]).ok()
}

/// result 字段判定：1 / "1" / true 均视为成功；ret_code=2 视为"已在线"
fn classify(v: Option<Value>, raw: &str) -> (bool, String) {
    if let Some(v) = v {
        let result_ok = match v.get("result") {
            Some(Value::Number(n)) => n.as_i64() == Some(1),
            Some(Value::String(s)) => matches!(s.trim(), "1" | "true" | "success" | "ok" | "成功"),
            Some(Value::Bool(b)) => *b,
            _ => false,
        };
        let ret_code = v.get("ret_code").and_then(|x| x.as_i64()).unwrap_or(-1);
        let msg = v
            .get("msg")
            .or_else(|| v.get("message"))
            .or_else(|| v.get("error"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if result_ok {
            let msg = if msg.is_empty() { "登录成功".to_string() } else { msg };
            return (true, msg);
        }
        if ret_code == 2 {
            return (true, "已在登录状态，无需重复认证".to_string());
        }
        return (false, if msg.is_empty() { "认证失败".to_string() } else { msg });
    }
    // JSON 解析失败时的宽松兜底
    let compact = raw.replace(' ', "");
    if compact.contains("\"result\":1") || compact.contains("\"result\":\"1\"") {
        return (true, "登录成功".to_string());
    }
    if compact.contains("成功") {
        return (true, "登录成功".to_string());
    }
    (false, "认证服务器响应异常".to_string())
}

fn read_body(resp: ureq::Response) -> String {
    resp.into_string().unwrap_or_default()
}

pub struct LoginOutcome {
    pub ok: bool,
    pub msg: String,
    pub ip: String,
    pub mac: String,
}

/// 执行登录（最多重试 3 次，仅对"连接失败"类错误重试）
pub fn do_login(agent: &ureq::Agent, probe: &ureq::Agent, cfg: &Config) -> LoginOutcome {
    let info = netinfo::detect(&cfg.portal);
    if info.ip.is_empty() {
        return LoginOutcome {
            ok: false,
            msg: "无法获取本机 IP，请确认已接入校园网".into(),
            ip: String::new(),
            mac: String::new(),
        };
    }
    let mac = if info.mac.is_empty() {
        "000000000000".to_string()
    } else {
        info.mac.clone()
    };

    // 预热：先访问门户首页建立会话 Cookie（尽力而为，失败不影响登录）
    let base = format!("http://{}/eportal/", cfg.portal);
    let _ = agent.get(&base).timeout(Duration::from_secs(5)).call();

    let url = build_login_url(cfg, &info.ip, &mac);

    // 提交认证请求（最多重试 3 次，仅对"连接失败"类错误重试）
    let mut portal_ok = false;
    let mut portal_msg = String::from("未知错误");
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(1500));
        }
        match agent.get(&url).timeout(Duration::from_secs(10)).call() {
            Ok(resp) => {
                let body = read_body(resp);
                let (ok, msg) = classify(parse_jsonp(&body), &body);
                portal_ok = ok;
                portal_msg = msg;
                break;
            }
            Err(ureq::Error::Status(_code, resp)) => {
                // 明确的业务应答（账号/密码错误等），重试无意义
                let body = read_body(resp);
                let (ok, msg) = classify(parse_jsonp(&body), &body);
                portal_ok = ok;
                portal_msg = msg;
                break;
            }
            Err(e) => {
                portal_msg = format!("连接认证服务器失败: {}", e);
            }
        }
    }

    // 最终判定：以「能否访问外网」为准。网关放行有延迟，最多复检 3 次
    let mut online = false;
    for i in 0..3 {
        std::thread::sleep(if i == 0 {
            Duration::from_millis(1000)
        } else {
            Duration::from_millis(2000)
        });
        if internet_reachable(probe, cfg) {
            online = true;
            break;
        }
    }

    let (ok, msg) = if online {
        (
            true,
            if portal_ok && !portal_msg.trim().is_empty() {
                portal_msg
            } else {
                "已连通互联网".to_string()
            },
        )
    } else if portal_ok {
        (false, "认证已通过，但仍无法访问外网".to_string())
    } else {
        (false, portal_msg)
    };

    LoginOutcome {
        ok,
        msg,
        ip: info.ip,
        mac,
    }
}

pub struct NetStatus {
    pub internet: bool,
    pub portal_ok: bool,
    pub detail: String,
}

fn is_portal_url(u: &str, portal: &str) -> bool {
    PORTAL_MARKERS.iter().any(|m| u.contains(m)) || (!portal.is_empty() && u.contains(portal))
}

/// 外网可达性判定：把响应内容与 Location 中出现认证页特征视为被拦截
pub fn internet_reachable(probe: &ureq::Agent, cfg: &Config) -> bool {
    for url in &cfg.probe_urls {
        match probe.get(url).call() {
            Ok(resp) => {
                let st = resp.status();
                let location = resp.header("location").unwrap_or("").to_string();
                if (300..400).contains(&st) {
                    if is_portal_url(&location, &cfg.portal) {
                        continue; // 被强制跳转到认证页 = 仍无外网
                    }
                    return true; // 站点自身跳转，说明流量未被拦截
                }
                if (200..300).contains(&st) {
                    let body = read_body(resp);
                    if is_portal_url(&body, &cfg.portal) {
                        continue; // 返回认证页内容 = 仍无外网
                    }
                    return true;
                }
            }
            Err(ureq::Error::Status(_code, resp)) => {
                let body = read_body(resp);
                if !is_portal_url(&body, &cfg.portal) {
                    return true; // 站点真实应答(即使 4xx/5xx)说明流量已放行
                }
            }
            Err(_) => continue,
        }
    }
    false
}

/// 双探测状态检测
pub fn check_status(agent: &ureq::Agent, probe: &ureq::Agent, cfg: &Config) -> NetStatus {
    // 1. 门户可达性
    let base = format!("http://{}/eportal/", cfg.portal);
    let portal_ok = agent.get(&base).timeout(Duration::from_secs(5)).call().is_ok();

    // 2. 外网可达性（多探测地址轮询）
    for url in &cfg.probe_urls {
        match probe.get(url).call() {
            Ok(resp) => {
                let st = resp.status();
                let location = resp.header("location").unwrap_or("").to_string();
                if (300..400).contains(&st) {
                    if is_portal_url(&location, &cfg.portal) {
                        return NetStatus {
                            internet: false,
                            portal_ok,
                            detail: "被强制跳转到认证页(未登录)".into(),
                        };
                    }
                    continue; // 站点自身跳转，换下一个探测点
                }
                if (200..300).contains(&st) {
                    let body = read_body(resp);
                    if is_portal_url(&body, &cfg.portal) {
                        return NetStatus {
                            internet: false,
                            portal_ok,
                            detail: "返回认证页内容(未登录)".into(),
                        };
                    }
                    return NetStatus {
                        internet: true,
                        portal_ok,
                        detail: "外网探测正常".into(),
                    };
                }
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = read_body(resp);
                if is_portal_url(&body, &cfg.portal) {
                    return NetStatus {
                        internet: false,
                        portal_ok,
                        detail: "返回认证页内容(未登录)".into(),
                    };
                }
                // 探测站点真实应答(即使 4xx/5xx)说明流量未被拦截
                let _ = code;
                return NetStatus {
                    internet: true,
                    portal_ok,
                    detail: "外网探测正常".into(),
                };
            }
            Err(_) => continue,
        }
    }

    NetStatus {
        internet: false,
        portal_ok,
        detail: "外网探测均未成功".into(),
    }
}
