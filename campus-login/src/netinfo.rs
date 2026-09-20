// 本机网络信息探测
// IP：UDP connect 到认证服务器（不发任何数据包），由内核路由表选出源 IP
// MAC：GetAdaptersInfo 枚举适配器，优先取持有该源 IP 的网卡
#![cfg(windows)]

use std::net::UdpSocket;

pub struct NetInfo {
    pub ip: String,
    pub mac: String,
}

struct Adapter {
    ips: Vec<String>,
    mac: String,
    if_type: u32,
}

/// 探测登录所需的 ip / mac；失败时给出空串兜底（portal 端一般仍可处理）
pub fn detect(portal_host_port: &str) -> NetInfo {
    let ip = local_ip_via_udp(portal_host_port).unwrap_or_default();
    let adapters = list_adapters();

    // 1. 精确匹配：持有该源 IP 的适配器
    if !ip.is_empty() {
        if let Some(a) = adapters
            .iter()
            .find(|a| a.ips.iter().any(|s| s == &ip) && !a.mac.is_empty())
        {
            return NetInfo {
                ip,
                mac: a.mac.clone(),
            };
        }
    }
    // 2. 兜底：选一张"看起来对"的物理网卡
    for want_type in [6u32, 71u32] {
        if let Some(a) = adapters.iter().find(|a| {
            a.if_type == want_type
                && !a.mac.is_empty()
                && a.ips
                    .iter()
                    .any(|ip| is_usable_unicast(ip))
        }) {
            return NetInfo {
                ip: a.ips.iter().find(|ip| is_usable_unicast(ip)).cloned().unwrap_or_default(),
                mac: a.mac.clone(),
            };
        }
    }
    if let Some(a) = adapters.iter().find(|a| {
        !a.mac.is_empty() && a.ips.iter().any(|ip| is_usable_unicast(ip))
    }) {
        return NetInfo {
            ip: a.ips.iter().find(|ip| is_usable_unicast(ip)).cloned().unwrap_or_default(),
            mac: a.mac.clone(),
        };
    }
    NetInfo {
        ip,
        mac: String::new(),
    }
}

fn is_usable_unicast(ip: &str) -> bool {
    !(ip.is_empty()
        || ip.starts_with("127.")
        || ip.starts_with("169.254.")
        || ip.starts_with("0."))
}

/// UDP connect 技巧：仅在本机内核里完成路由选择，不产生网络流量
fn local_ip_via_udp(portal: &str) -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect(portal).ok()?;
    let local = sock.local_addr().ok()?;
    match local {
        std::net::SocketAddr::V4(v4) => Some(v4.ip().to_string()),
        _ => None,
    }
}

fn cstr(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

fn mac_to_hex(addr: &[u8], len: usize) -> String {
    let n = len.min(6);
    let bytes = &addr[..n];
    if bytes.iter().all(|&b| b == 0) {
        return String::new();
    }
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn list_adapters() -> Vec<Adapter> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetAdaptersInfo, IP_ADAPTER_INFO};

    let mut out = Vec::new();
    unsafe {
        let mut size: u32 = 0;
        // 第一次调用：获取所需缓冲区大小（返回 111=ERROR_BUFFER_OVERFLOW 属正常）
        let _ = GetAdaptersInfo(std::ptr::null_mut(), &mut size);
        if size == 0 {
            return out;
        }
        let mut buf: Vec<u8> = vec![0u8; size as usize];
        let ret = GetAdaptersInfo(buf.as_mut_ptr() as *mut IP_ADAPTER_INFO, &mut size);
        if ret != 0 {
            return out;
        }
        let mut cur = buf.as_mut_ptr() as *const IP_ADAPTER_INFO;
        let mut guard = 0;
        while !cur.is_null() && guard < 64 {
            guard += 1;
            let a = &*cur;
            let mac = mac_to_hex(&a.Address, a.AddressLength as usize);
            let mut ips = Vec::new();
            let mut p: *const windows_sys::Win32::NetworkManagement::IpHelper::IP_ADDR_STRING =
                &a.IpAddressList;
            let mut pguard = 0;
            while !p.is_null() && pguard < 16 {
                pguard += 1;
                let entry = &*p;
                let ip = cstr(&entry.IpAddress.String);
                if !ip.is_empty() {
                    ips.push(ip);
                }
                p = entry.Next;
            }
            out.push(Adapter {
                ips,
                mac,
                if_type: a.Type,
            });
            cur = a.Next;
        }
    }
    out
}
