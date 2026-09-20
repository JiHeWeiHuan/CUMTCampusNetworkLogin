# 校园网自动登录（campus-login）

一个常驻系统托盘的校园网自动登录工具。单文件 exe（**1.05 MB**），无需安装、无外部运行时依赖。

---

## 一、快速使用

1. 把 `campus-login.exe` 放到任意目录（例如 `D:\Tools\campus-login\`）。
2. 双击运行。**首次运行会自动在同目录生成 `config.txt`**，并自动完成开机自启注册。
3. 若生成的配置里账号密码不符，用记事本打开 `config.txt` 改成自己的，保存后右键托盘图标点 **一键登录** 即可生效（无需重启程序）。
4. 右下角托盘出现一个图标，右键即为菜单。

> 建议把 exe 固定存放在一个不常变动的目录（开机自启记录的是该路径；若移动文件，下次启动会自动更新注册表项）。

## 二、托盘菜单（右键）

| 菜单项 | 行为 |
| --- | --- |
| **一键登录** | 重新读取 `config.txt` → 自动探测本机 IP/MAC → 提交认证请求，并以「能否访问 baidu.com」判定是否真正连通互联网；结果直接显示在"当前状态"里 |
| **检测状态** | 主动探测"认证服务器可达性"+"外网可达性"，综合判定当前状态 |
| **当前状态** | 实时状态显示（灰色不可点）：`已连接互联网` / `未登录校园网（认证服务器可达但外网不可达）` / `未接入校园网` / `登录失败：…` / `请编辑 config.txt` |
| **退出后台** | 退出程序（不改动开机自启设置） |

**图标颜色即状态**：绿色=已连接，红色=未登录/登录失败，黄色=正在登录或检测，灰色=未接入校园网/配置缺失。
**左键双击**图标 = 一键登录。

## 三、自动行为

- **开机自动登录**：注册表 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 下 `CampusNetAutoLogin`，幂等写入。
- **启动即检测**：程序启动后立即探测一次，若发现未登录且 `auto_login=1`，自动完成登录。
- **断线自动重连**：每 relogin_interval 秒（默认 1800s = 30 分钟）后台探测一次，确认掉线才重新登录，避免对认证服务器造成压力、也避免反复触发账号风控。

## 四、config.txt 参数

```ini
portal=                    # 认证服务器 主机:端口，端口省略默认 801（写完整 URL 也会自动规整）
username=                  # 账号；运营商后缀直接带上（@cmcc 移动 / @telecom 电信 / @unicom 联通）
password=                  # 密码（含特殊字符无需转义，程序会做百分号编码）
ac_name=NAS                # AC 名称，一般保持默认（留空则用 NAS）
auto_login=1               # 1=开机/启动自动登录  0=只显示状态不自动登录
relogin_interval=1800      # 自动检测间隔(秒)，默认 1800 = 30 分钟，最小 30
```

可选补充项：

- `isp=cmcc`：当 `username` 不含 `@` 时，自动拼接为 `账号@cmcc`。
- `probe_urls=http://www.baidu.com/`：外网探测地址，`|` 分隔，按顺序尝试；默认只探测 baidu.com，能访问即视为已连通互联网。
- 键名宽松：`user/pwd/server/账号/密码/间隔` 等别名均被识别；支持 `#` 注释、`key=value` 两侧空格、值外层引号。
- 编码宽松：UTF-8 / 记事本 UTF-16LE / GBK 均可正常读取。

## 五、工作日志

同目录 `campus-login.log`，记录每次启动、检测、登录结果与 IP/MAC。超过 512 KB 自动重建。排查问题时先看它。

## 六、实现与可靠性要点

- **协议**：`GET /eportal/?c=Portal&a=login&login_method=1&user_account=…&user_password=…&wlan_user_ip=…&wlan_user_mac=…&wlan_ac_ip=&wlan_ac_name=…&jsVersion=3.0&_=…`，响应为 JSONP，取 `result` 判定（`1`/`"1"`/`true` 为成功；`ret_code=2` 视为"已在线"）。
- **IP 探测**：UDP `connect` 到认证服务器（不发送任何数据包），由内核路由表选出真实出口 IP，避免多网卡/VPN 选错。
- **MAC 探测**：`GetAdaptersInfo` 枚举网卡，优先返回持有上述出口 IP 的那张网卡，再退回物理网卡，最终格式化为 12 位小写十六进制（与门户要求一致）。
- **状态判定**：以「能否访问外网」为核心。访问 `http://www.baidu.com/`（默认探测地址，可在 config.txt 用 probe_urls 追加）成功返回即视为已连通互联网；若被强制跳转到门户（响应体或 Location 含 eportal/wlan_user_ip/c=Portal 特征，或指向 portal 主机）则判定为未登录；两者都不成立则视为未接入校园网。
- **不会闪退**：所有网络请求有超时（连接 4s / 总 10s），网络工作在后台线程执行、主线程只跑消息循环，登录失败自动重试 3 次，登录提交后还会就外网连通性复检最多 3 次（间隔 1s/2s/2s）再下结论；线程体与主流程均用 `catch_unwind` 兜底，异常只记日志不崩进程；无 `unwrap` 于任何用户输入/网络路径。
- **零控制台**：以 `windows_subsystem = "windows"` 编译，双击无黑框。

## 七、重新编译（可选）

源码位于 `campus-login/`，需要 Rust 工具链（`x86_64-pc-windows-gnu`）。

```cmd
cd campus-login
build.cmd
```

> 注意：若源码路径含中文，MinGW 链接器在处理某些目标文件时会失败。`build.cmd` 已通过把 `CARGO_TARGET_DIR` 指向纯 ASCII 的临时目录规避该问题。产物为 `%CARGO_TARGET_DIR%\<triple>\release\campus-login.exe`。

依赖：`ureq`(HTTP) / `serde_json`(解析) / `tray-icon`+`muda`(托盘与菜单) / `windows-sys`(消息循环与网卡枚举) / `winreg`(开机自启)。
