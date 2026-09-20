// 校园网自动登录 —— 托盘常驻程序
// 逻辑总览：
//   1. 启动：初始化日志 -> 读取 config.txt -> 注册开机自启 -> 建托盘
//   2. 立即做一次状态探测(auto)：未登录则自动登录
//   3. 消息循环：定时泵菜单事件/托盘事件/工作线程结果；周期性探测并按需自动重连
//   4. 所有网络工作在后台线程执行，主线程永不阻塞；panic 全部兜底，不闪退
#![windows_subsystem = "windows"]

mod autostart;
mod config;
mod logger;
mod netinfo;
mod portal;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MessageBoxW, PostQuitMessage, PostThreadMessageW, SetTimer,
    TranslateMessage, MSG, MB_ICONERROR,
};

const WM_WAKE: u32 = 0x0400 + 1; // WM_APP + 1：工作线程唤醒主循环
const TIMER_ID: usize = 1;

const COLOR_ONLINE: [u8; 3] = [46, 204, 113]; // 绿
const COLOR_OFFLINE: [u8; 3] = [231, 76, 60]; // 红
const COLOR_WAIT: [u8; 3] = [241, 196, 15]; // 黄
const COLOR_ERR: [u8; 3] = [127, 140, 141]; // 灰

static BUSY: AtomicBool = AtomicBool::new(false);
static MAIN_TID: AtomicU32 = AtomicU32::new(0);

enum WMsg {
    Login {
        ok: bool,
        msg: String,
        ip: String,
        mac: String,
        source: &'static str,
    },
    Checked {
        internet: bool,
        portal_ok: bool,
        detail: String,
        auto: bool,
    },
}

fn wake_main() {
    let tid = MAIN_TID.load(Ordering::SeqCst);
    if tid != 0 {
        unsafe {
            PostThreadMessageW(tid, WM_WAKE, 0, 0);
        }
    }
}

fn panic_msg(e: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = e.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown".to_string()
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n).collect();
        format!("{}…", t)
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 单实例检查：已存在同名互斥体则说明程序已在运行
fn already_running() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name = wide("Local\\campus-login-single-instance");
    unsafe {
        let h = CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr());
        if h == 0 {
            return false; // 创建失败时不阻止启动
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return true;
        }
        // 不关闭句柄：使其在进程存活期间持续有效（HANDLE 为裸 isize，无析构）
        let _keep_alive = h;
        false
    }
}

fn fatal_box(title: &str, text: &str) {
    let t = wide(title);
    let b = wide(text);
    unsafe {
        MessageBoxW(0, b.as_ptr(), t.as_ptr(), MB_ICONERROR);
    }
}

/// 程序化绘制 64x64 托盘图标：圆角底 + 白色 wifi 弧线，颜色随状态变化
fn make_icon(color: [u8; 3]) -> Icon {
    const S: usize = 64;
    let mut px = vec![0u8; S * S * 4];
    let half = 30.0f32;
    let corner = 13.0f32;
    for y in 0..S {
        for x in 0..S {
            let dx = x as f32 - 32.0;
            let dy = y as f32 - 32.0;
            // 圆角矩形 SDF + 简易抗锯齿
            let qx = dx.abs() - (half - corner);
            let qy = dy.abs() - (half - corner);
            let outside = f32::max(qx, 0.0).hypot(f32::max(qy, 0.0));
            let inside = f32::min(f32::max(qx, qy), 0.0);
            let d = outside + inside - corner;
            let coverage = ((0.5 - d) / 1.5).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            // wifi 图案：中心(32,44)，三条向上弧线 + 一个圆点
            let rx = dx;
            let ry = 44.0 - y as f32;
            let r = rx.hypot(ry);
            let deg = ry.atan2(rx).to_degrees();
            let mut white = false;
            if deg > 25.0 && deg < 155.0 {
                for band in [12.0f32, 21.0, 30.0] {
                    if (r - band).abs() <= 2.6 {
                        white = true;
                    }
                }
            }
            if r <= 4.2 {
                white = true;
            }
            let (cr, cg, cb) = if white {
                (255u8, 255u8, 255u8)
            } else {
                (color[0], color[1], color[2])
            };
            let i = (y * S + x) * 4;
            px[i] = cr;
            px[i + 1] = cg;
            px[i + 2] = cb;
            px[i + 3] = (coverage * 255.0) as u8;
        }
    }
    match Icon::from_rgba(px, S as u32, S as u32) {
        Ok(icon) => icon,
        Err(_) => Icon::from_rgba(vec![255, 0, 0, 255], 1, 1).expect("icon 1x1 不可能失败"),
    }
}

struct App {
    tray: TrayIcon,
    id_login: MenuId,
    id_check: MenuId,
    id_quit: MenuId,
    mi_status: MenuItem,
    agent: ureq::Agent,
    probe: ureq::Agent,
    cfg: Option<config::Config>,
    rx: Receiver<WMsg>,
    tx: Sender<WMsg>,
    last_check: Instant,
}

impl App {
    fn set_status(&self, text: &str, color: [u8; 3], tooltip: &str) {
        self.mi_status.set_text(format!("当前状态：{}", text));
        let _ = self.tray.set_icon(Some(make_icon(color)));
        let _ = self.tray.set_tooltip(Some(tooltip));
    }

    fn trigger_login(&mut self, source: &'static str) {
        // 手动登录时重新读取配置，便于改完 config.txt 直接生效
        if source != "自动重连" {
            match config::Config::load_or_create() {
                Ok(c) => {
                    logger::write(
                        "INFO",
                        &format!("配置(重)加载成功 portal={} user={}", c.portal, c.username),
                    );
                    self.cfg = Some(c);
                }
                Err(e) => {
                    logger::write("WARN", &e);
                    self.cfg = None;
                    self.set_status("请编辑 config.txt", COLOR_ERR, &truncate(&e, 60));
                    return;
                }
            }
        }
        let cfg = match &self.cfg {
            Some(c) => c.clone(),
            None => {
                self.set_status("请编辑 config.txt", COLOR_ERR, "缺少账号配置");
                return;
            }
        };
        if BUSY.load(Ordering::SeqCst) {
            return;
        }
        BUSY.store(true, Ordering::SeqCst);
        self.last_check = Instant::now();
        self.set_status("正在登录…", COLOR_WAIT, "正在登录校园网…");
        let tx = self.tx.clone();
        let agent = self.agent.clone();
        let probe = self.probe.clone();
        std::thread::spawn(move || {
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                portal::do_login(&agent, &probe, &cfg)
            }));
            let msg = match out {
                Ok(o) => WMsg::Login {
                    ok: o.ok,
                    msg: o.msg,
                    ip: o.ip,
                    mac: o.mac,
                    source,
                },
                Err(e) => {
                    logger::write("ERROR", &format!("登录线程异常: {}", panic_msg(e.as_ref())));
                    WMsg::Login {
                        ok: false,
                        msg: "内部错误".into(),
                        ip: String::new(),
                        mac: String::new(),
                        source,
                    }
                }
            };
            let _ = tx.send(msg);
            BUSY.store(false, Ordering::SeqCst);
            wake_main();
        });
    }

    fn trigger_check(&mut self, auto: bool) {
        if self.cfg.is_none() {
            self.last_check = Instant::now();
            return;
        }
        if BUSY.load(Ordering::SeqCst) {
            return;
        }
        BUSY.store(true, Ordering::SeqCst);
        self.last_check = Instant::now();
        if !auto {
            self.set_status("正在检测…", COLOR_WAIT, "正在检测网络状态…");
        }
        let tx = self.tx.clone();
        let agent = self.agent.clone();
        let probe = self.probe.clone();
        let cfg = self.cfg.clone().unwrap();
        std::thread::spawn(move || {
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                portal::check_status(&agent, &probe, &cfg)
            }));
            let msg = match out {
                Ok(s) => WMsg::Checked {
                    internet: s.internet,
                    portal_ok: s.portal_ok,
                    detail: s.detail,
                    auto,
                },
                Err(e) => {
                    logger::write("ERROR", &format!("检测线程异常: {}", panic_msg(e.as_ref())));
                    WMsg::Checked {
                        internet: false,
                        portal_ok: false,
                        detail: "内部错误".into(),
                        auto,
                    }
                }
            };
            let _ = tx.send(msg);
            BUSY.store(false, Ordering::SeqCst);
            wake_main();
        });
    }

    fn handle(&mut self, m: WMsg) {
        match m {
            WMsg::Login {
                ok,
                msg,
                ip,
                mac,
                source,
            } => {
                logger::write(
                    if ok { "INFO" } else { "WARN" },
                    &format!(
                        "[{}] 登录结果: {} (ip={} mac={})",
                        source, msg, ip, mac
                    ),
                );
                if ok {
                    let tip = format!("已连通互联网 {}", if ip.is_empty() { "-" } else { &ip });
                    self.set_status(
                        &format!("已连通互联网 ({})", if ip.is_empty() { "-" } else { &ip }),
                        COLOR_ONLINE,
                        &tip,
                    );
                } else {
                    let tip = format!("登录失败：{}", msg);
                    self.set_status(&format!("登录失败：{}", truncate(&msg, 32)), COLOR_OFFLINE, &tip);
                }
                self.last_check = Instant::now();
            }
            WMsg::Checked {
                internet,
                portal_ok,
                detail,
                auto,
            } => {
                logger::write(
                    "INFO",
                    &format!(
                        "检测[{}]: internet={} portal={} ({})",
                        if auto { "自动" } else { "手动" },
                        internet,
                        portal_ok,
                        detail
                    ),
                );
                if internet {
                    self.set_status("已连接互联网", COLOR_ONLINE, "校园网已连接");
                } else if portal_ok {
                    self.set_status("未登录校园网", COLOR_OFFLINE, "校园网未登录(点击一键登录)");
                    if auto {
                        let do_login = self
                            .cfg
                            .as_ref()
                            .map(|c| c.auto_login)
                            .unwrap_or(false);
                        if do_login {
                            self.trigger_login("自动重连");
                            return; // trigger_login 内部已更新 last_check
                        }
                    }
                } else {
                    self.set_status("未接入校园网", COLOR_ERR, "未接入校园网(认证服务器不可达)");
                }
                self.last_check = Instant::now();
            }
        }
    }

    fn pump(&mut self) {
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == self.id_login {
                self.trigger_login("手动");
            } else if ev.id == self.id_check {
                self.trigger_check(false);
            } else if ev.id == self.id_quit {
                logger::write("INFO", "用户选择退出后台");
                unsafe {
                    PostQuitMessage(0);
                }
                return;
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } = ev
            {
                self.trigger_login("双击");
            }
        }
        while let Ok(m) = self.rx.try_recv() {
            self.handle(m);
        }
        // 周期自动检测 + 按需自动重连
        let interval = self
            .cfg
            .as_ref()
            .map(|c| c.relogin_interval)
            .unwrap_or(1800);
        if self.cfg.is_some()
            && !BUSY.load(Ordering::SeqCst)
            && self.last_check.elapsed() >= Duration::from_secs(interval)
        {
            self.trigger_check(true);
        }
    }
}

fn main() {
    logger::init();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
    if let Err(e) = result {
        let m = panic_msg(e.as_ref());
        logger::write("FATAL", &format!("主流程异常退出: {}", m));
        fatal_box("校园网自动登录", &format!("程序异常退出：{}\n详情见 campus-login.log", m));
    }
}

fn run() {
    logger::write("INFO", "========== 程序启动 ==========");

    if already_running() {
        logger::write("INFO", "检测到已有实例在运行，本次启动退出");
        return;
    }

    let cfg = match config::Config::load_or_create() {
        Ok(c) => {
            logger::write(
                "INFO",
                &format!(
                    "配置加载成功 portal={} user={} interval={}s",
                    c.portal, c.username, c.relogin_interval
                ),
            );
            Some(c)
        }
        Err(e) => {
            logger::write("WARN", &e);
            None
        }
    };

    match autostart::enable() {
        Ok(_) => logger::write("INFO", "开机自启已就绪"),
        Err(e) => logger::write("WARN", &format!("开机自启设置失败: {}", e)),
    }

    let agent = portal::build_agent();
    let probe = portal::build_probe_agent();
    let (tx, rx) = channel::<WMsg>();
    MAIN_TID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);

    let menu = Menu::new();
    let mi_login = MenuItem::with_id("login", "一键登录", true, None);
    let mi_check = MenuItem::with_id("check", "检测状态", true, None);
    let mi_status = MenuItem::with_id("status", "当前状态：启动中…", false, None);
    let sep1 = PredefinedMenuItem::separator();
    let sep2 = PredefinedMenuItem::separator();
    let mi_quit = MenuItem::with_id("quit", "退出后台", true, None);
    let _ = menu.append(&mi_login);
    let _ = menu.append(&mi_check);
    let _ = menu.append(&sep1);
    let _ = menu.append(&mi_status);
    let _ = menu.append(&sep2);
    let _ = menu.append(&mi_quit);

    let tray = match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("校园网自动登录")
        .with_icon(make_icon(COLOR_WAIT))
        .with_menu_on_left_click(false)
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            logger::write("FATAL", &format!("托盘图标创建失败: {}", e));
            fatal_box("校园网自动登录", &format!("托盘图标创建失败：{}", e));
            return;
        }
    };

    let mut app = App {
        tray,
        id_login: mi_login.id().clone(),
        id_check: mi_check.id().clone(),
        id_quit: mi_quit.id().clone(),
        mi_status,
        agent,
        probe,
        cfg,
        rx,
        tx,
        last_check: Instant::now(),
    };

    // 启动即探测一次（auto=true：若未登录则自动登录）
    app.trigger_check(true);
    app.set_status("初始化中…", COLOR_WAIT, "校园网自动登录已启动");

    unsafe {
        SetTimer(0, TIMER_ID, 300, None);
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let r = GetMessageW(&mut msg, 0, 0, 0);
            if r == 0 {
                break; // WM_QUIT
            }
            if r == -1 {
                std::thread::sleep(Duration::from_millis(10));
                app.pump();
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            app.pump();
        }
        windows_sys::Win32::UI::WindowsAndMessaging::KillTimer(0, TIMER_ID);
    }

    logger::write("INFO", "========== 程序退出 ==========");
}
