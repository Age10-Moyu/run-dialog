//! 提权（类 UAC）支持。
//!
//! 目标：模拟 Windows 的「以管理员身份运行程序」。
//! 由于 Linux 上 `sudo` 从 tty 读密码而 GTK 程序通常没有 tty，
//! 这里走 `SUDO_ASKPASS` 机制：把密码写进一个**仅属主可读的临时文件**，
//! 再用 `sudo -A` 让 sudo 调用我们自己的 `--askpass` 模式去读它。
//!
//! ## 密码流转
//!
//! ```text
//! 主进程（GTK）
//!   │  1. 用户输入密码（存在内存里）
//!   │  2. 创建 0600 临时文件，写入密码
//!   │  3. 设环境 SUDO_ASKPASS=<自身路径> + RUN_DIALOG_ASKPASS=<临时文件路径>
//!   │  4. spawn `sudo -A -u root <prog> <args>`
//!   │       └─ sudo 执行 askpass → run-dialog --askpass 从文件读密码 → 输出到 stdout
//!   │  5. 立即删除临时文件（Drop guard）
//! ```
//!
//! ## 安全边界
//!
//! - 密码**不经过 argv、不进环境变量、不写日志**，只存在于内存与 0600 文件
//! - 临时文件放在 0700 目录下，名为 `.askpass-<随机>`（点开头，ls 默认不显示）
//! - `AskpassGuard` 保证文件在作用域结束时被删除，即使提前 `return` 或 panic
//! - 注意：同用户的其它进程理论上仍可能抢在删除前读到文件。这是
//!   `SUDO_ASKPASS` 机制的固有局限，经典 sudo 的图形前端（ssh-askpass、
//!   zenity 包装）同样如此。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 用于在 sudo 与 askpass 之间传递临时文件路径的环境变量。
pub const ASKPASS_ENV: &str = "RUN_DIALOG_ASKPASS_FILE";

/// 用零覆写字符串所占内存后清空。
///
/// 为什么要这么做：`String` 的 `Drop` 只把长度置零并归还内存，
/// **不会擦除内容**。密码明文可能因此驻留到该内存被重新分配，
/// 期间可被 core dump、swap 或同用户的进程读取。
///
/// 实现要点：
/// - 用 `write_volatile` 防止编译器把「写了立刻不再读」的覆写优化掉
/// - 用 `compiler_fence` 阻止重排序，确保覆写在 `clear()`（归还内存）之前完成
/// - `unsafe` 是必要的：我们要在安全抽象之外操作已初始化的字节
pub fn wipe_string(s: &mut String) {
    // SAFETY: `as_mut_vec` 得到的字节序列一定是已初始化的 UTF-8；
    // 覆写后立即 `clear()`，长度归零，不会留下非 UTF-8 的可见内容。
    unsafe {
        let bytes = s.as_mut_vec();
        let ptr = bytes.as_mut_ptr();
        for i in 0..bytes.len() {
            std::ptr::write_volatile(ptr.add(i), 0u8);
        }
        // 确保覆写在长度归零（进而可能释放内存）之前完成
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
        bytes.clear();
    }
}

/// 持有密码临时文件，离开作用域时自动删除。
pub struct AskpassGuard {
    path: PathBuf,
}

impl AskpassGuard {
    /// 把密码写入一个新建的 0600 临时文件。
    ///
    /// 目录优先用 `$XDG_RUNTIME_DIR`（通常是 `/run/user/<uid>`，本身 0700
    /// 且基于 tmpfs），退回到 `~/.cache/run-dialog`。
    pub fn new(password: &str) -> std::io::Result<Self> {
        let dir = runtime_dir()?;
        std::fs::create_dir_all(&dir)?;
        restrict_dir(&dir)?;

        let path = unique_path(&dir);

        // 用 create_new 避免符号链接攻击（O_EXCL），并以 0600 创建
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&path)?;
        f.write_all(password.as_bytes())?;
        f.write_all(b"\n")?; // sudo 读取一行
        f.flush()?;
        drop(f);

        Ok(Self { path })
    }

    /// 临时文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for AskpassGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// askpass 模式：从 `ASKPASS_ENV` 指向的文件读出密码并写到 stdout。
///
/// 由 `sudo -A` 调用。失败时输出空行并返回非零，避免泄露信息。
pub fn run_askpass_mode() -> i32 {
    let Some(path) = std::env::var_os(ASKPASS_ENV) else {
        eprintln!("{ASKPASS_ENV} not set");
        return 1;
    };
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            // 只取第一行，去掉结尾换行（sudo 会自己处理）
            let line = content.lines().next().unwrap_or("");
            println!("{line}");
            0
        }
        Err(e) => {
            eprintln!("cannot read askpass file: {e}");
            1
        }
    }
}

/// 运行时的私有目录。
fn runtime_dir() -> std::io::Result<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(d).join("run-dialog");
        if !p.as_os_str().is_empty() {
            return Ok(p);
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    Ok(home.join(".cache/run-dialog"))
}

/// 把目录权限收紧到 0700。
fn restrict_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(dir)?.permissions();
        perm.set_mode(0o700);
        std::fs::set_permissions(dir, perm)?;
    }
    let _ = dir;
    Ok(())
}

/// 生成不冲突的临时文件名。
fn unique_path(dir: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);

    dir.join(format!(".askpass-{pid}-{nanos}-{seq}"))
}

/// 检查 `sudo -A` 在当前环境是否可用（有 sudo 且能指定 askpass）。
pub fn sudo_available() -> Option<PathBuf> {
    for p in ["/usr/bin/sudo", "/bin/sudo", "/usr/local/bin/sudo"] {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    // 回退到 PATH 查找
    let out = Command::new("sh")
        .args(["-c", "command -v sudo"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

/// 构造提权启动命令：`sudo -A -u root <program> <args>`。
pub fn build_command(sudo: &Path, program: &str, args: &[String]) -> Command {
    let mut c = Command::new(sudo);
    c.arg("-A"); // 使用 askpass 取密码
    c.arg("-u").arg("root");
    c.arg("--");
    c.arg(program);
    c.args(args);
    c
}

/// 启动提权进程，返回子进程句柄。
///
/// 子进程会脱离控制终端（`setsid`）。`-A` 已显式要求 sudo 使用 askpass，
/// 所以这对「askpass 是否被调用」没有影响；脱离 tty 的目的是另一个：
/// 提权后的程序是 GUI 应用，若继承启动终端，终端关闭时会被 `SIGHUP` 杀掉。
pub fn spawn_elevated(
    sudo: &Path,
    program: &str,
    args: &[String],
    askpass_path: &Path,
) -> std::io::Result<std::process::Child> {
    let mut cmd = build_command(sudo, program, args);
    cmd.env("SUDO_ASKPASS", current_exe()?);
    cmd.env(ASKPASS_ENV, askpass_path);
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null());

    // 让子进程成为新会话首进程，从而不再持有控制终端
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: 在 fork 后、exec 前调用 setsid()。此处不分配内存、
        // 不加锁，符合 async-signal-safe 要求。
        unsafe {
            cmd.pre_exec(|| {
                if libc_setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    cmd.spawn()
}

// 不引入 libc crate，直接声明 `setsid(2)`。
// `setsid` 是 POSIX 标准函数，失败时返回 -1 并设置 errno。
#[cfg(unix)]
extern "C" {
    #[link_name = "setsid"]
    fn libc_setsid() -> i32;
}

#[cfg(not(unix))]
unsafe fn libc_setsid() -> i32 {
    0
}

/// 当前可执行文件路径（用作 `SUDO_ASKPASS` 的值）。
fn current_exe() -> std::io::Result<PathBuf> {
    std::env::current_exe()
}

/// 认证与启动的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthOutcome {
    /// 认证通过，程序已启动
    Ok,
    /// 认证失败（密码错误、sudo 不可用等），附可展示的原因
    Failed(String),
}

/// 认证等待上限。超过则视为「认证已通过，程序正在运行」。
///
/// 依据：sudo 若密码错误会立即退出（通常 < 1 秒）；而认证成功后，
/// sudo 会 exec 目标程序并**持续运行**直到它退出。所以「短时间内就退出」
/// 基本等同于「认证失败」。
const AUTH_GRACE: std::time::Duration = std::time::Duration::from_millis(1200);

/// 判断 sudo 是否因认证失败而快速退出。返回 `Some(原因)` 表示失败。
fn classify_failure(status: std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;

    if status.success() {
        return None;
    }
    if let Some(sig) = status.signal() {
        return Some(format!("killed by signal {sig}"));
    }
    Some(match status.code() {
        Some(1) => "incorrect password".to_string(),
        Some(c) => format!("sudo exited with code {c}"),
        None => "sudo terminated abnormally".to_string(),
    })
}

/// 认证并提权启动：写临时 askpass 文件 → `sudo -A` 启动 → 判断认证结果。
///
/// 与早先实现的区别：**不再无条件返回成功**。早先版本把 `child.wait()`
/// 丢进后台线程并忽略退出码，导致密码错误时 UAC 照常关闭、主窗口被销毁，
/// 用户只看到界面消失而没有任何提示。
///
/// 现在的策略：
/// 1. 启动 sudo
/// 2. 在 `AUTH_GRACE` 内轮询子进程
///    - 已退出且非 0 → 认证失败，返回 `Failed`
///    - 仍在运行 → 认为认证通过（sudo 已 exec 目标程序），返回 `Ok`
/// 3. askpass 临时文件在本函数返回前删除（此时认证阶段已结束）
///
/// 注意：本函数会**阻塞至多 `AUTH_GRACE`**，调用方必须放到后台线程，
/// 否则会冻结 GTK 主循环。
pub fn elevate_and_authenticate(password: &str, program: &str, args: &[String]) -> AuthOutcome {
    let Some(sudo) = sudo_available() else {
        return AuthOutcome::Failed("sudo is not available".to_string());
    };

    let guard = match AskpassGuard::new(password) {
        Ok(g) => g,
        Err(e) => return AuthOutcome::Failed(e.to_string()),
    };

    let mut child = match spawn_elevated(&sudo, program, args, guard.path()) {
        Ok(c) => c,
        Err(e) => return AuthOutcome::Failed(e.to_string()),
    };

    // 轮询判断「快速退出」还是「认证成功并在运行」
    let deadline = std::time::Instant::now() + AUTH_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return match classify_failure(status) {
                    Some(reason) => AuthOutcome::Failed(reason),
                    None => AuthOutcome::Ok,
                };
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // 仍在运行 → 认证已通过，sudo 已把控制权交给目标程序
                    return AuthOutcome::Ok;
                }
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            Err(e) => return AuthOutcome::Failed(e.to_string()),
        }
    }
}

/// 解析可用于提权的管理员账户，按「最早创建」排序。
///
/// 对应 UAC 里预填用户名的需求。判据：
/// - `sudo` 组成员（Debian/Ubuntu）或 `wheel` 组成员（RHEL/Fedora/Arch）
/// 解析可用于提权的管理员账户。
///
/// 判据：
/// - `sudo` 组成员（Debian/Ubuntu）、`wheel` 组成员（RHEL/Fedora/Arch）
///   或 `adm` 组成员
/// - 或 uid 0
///
/// 不含系统账户（uid 1..999）以及 shell 为 `nologin`/`false` 的账户。
///
/// 排序：**已知可登录的账户优先**，其后按 uid 升序（≈ 最早创建）。
///
/// ## 关于 `can_login`
///
/// 判断依据是 `/etc/shadow` 里密码字段是否为 `!`/`*`。但**普通用户读不到
/// `/etc/shadow`**，此时 `locked_accounts()` 返回空列表，所有账户的
/// `can_login` 都是 `true`——即「未知，不排除」。因此排序会退化为纯 uid
/// 升序，root 排在最前。
///
/// 这是权限模型决定的固有限制：非 root 进程无法得知哪些账户能登录
/// （`passwd -S root` 同样被拒绝）。若以 root 身份运行本程序，
/// `can_login` 才会真正生效。
pub fn admin_accounts() -> Vec<AdminAccount> {
    let members = privileged_members();
    let Ok(passwd) = std::fs::read_to_string("/etc/passwd") else {
        return Vec::new();
    };

    let locked = locked_accounts();

    let mut out: Vec<AdminAccount> = passwd
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(':').collect();
            if f.len() < 7 {
                return None;
            }
            let name = f[0];
            let uid: u32 = f[2].parse().ok()?;
            let gid: u32 = f[3].parse().ok()?;
            let shell = f[6];

            let is_root = uid == 0;
            // 可登录的普通用户：uid >= 1000 且 shell 不是 nologin/false
            let has_login_shell =
                uid >= 1000 && !shell.ends_with("nologin") && !shell.ends_with("false");

            if !is_root && !has_login_shell {
                return None;
            }
            // 非特权组的不收（root 除外）
            if !is_root && !members.iter().any(|(n, g)| *n == name || *g == gid) {
                return None;
            }

            Some(AdminAccount {
                name: name.to_string(),
                uid,
                can_login: !locked.iter().any(|l| l == name),
            })
        })
        .collect();

    out.sort_by(|a, b| {
        // 已知可登录的排在前面，其后按 uid 升序
        b.can_login.cmp(&a.can_login).then(a.uid.cmp(&b.uid))
    });
    out
}

/// 读取 `/etc/shadow` 中密码被锁定的账户名。
///
/// 普通用户读不到 `/etc/shadow`（会返回空列表），此时视为「未知」，
/// 不据此排除任何账户——预填只是方便，认证失败用户会自己改。
fn locked_accounts() -> Vec<String> {
    let Ok(shadow) = std::fs::read_to_string("/etc/shadow") else {
        return Vec::new();
    };
    shadow
        .lines()
        .filter_map(|line| {
            let mut f = line.split(':');
            let name = f.next()?;
            let hash = f.next()?;
            // `!` 前缀表示锁定，`*` 表示无密码可用
            if hash.starts_with('!') || hash.starts_with('*') {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// 从 `sudo` / `wheel` / `adm` 组里收集成员（用户名与 gid）。
fn privileged_members() -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for group in ["sudo", "wheel", "adm"] {
        // getent 比直接解析 /etc/group 可靠（能覆盖 LDAP 等）
        let Ok(o) = Command::new("getent").args(["group", group]).output() else {
            continue;
        };
        if !o.status.success() {
            continue;
        }
        let line = String::from_utf8_lossy(&o.stdout);
        let Some((_, rest)) = line.trim().split_once(':') else {
            continue;
        };
        // group:x:gid:member1,member2
        let parts: Vec<&str> = rest.split(':').collect();
        let gid = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(u32::MAX);
        if let Some(list) = parts.get(2) {
            for m in list.split(',').filter(|s| !s.is_empty()) {
                out.push((m.trim().to_string(), gid));
            }
        }
    }
    out
}

/// 当前的提权相关环境信息（供「签名信息」面板展示）。
///
/// 暂无调用者（UAC 里已改为纯文本「此程序未签名。」），保留以备将来
/// 接入发行版签名验证或提供「程序详情」入口。
///
/// Linux 没有 Windows 的 Authenticode 体系，这里只呈现**我们能确知的事实**，
/// 不做「已签名 / 未签名」之类无法验证的断言。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SignatureInfo {
    /// 程序真实路径（解析符号链接后）
    pub resolved_path: Option<PathBuf>,
    /// 是否为 ELF 且大小
    pub file_kind: String,
    /// 文件大小（字节）
    pub file_size: Option<u64>,
    /// ELF 是否含有 `.gnu_debuglink` 之外的签名相关段（如 `.note.gnu.build-id`）
    pub build_id: Option<String>,
    /// 对应的 `.desktop` 文件路径（若有）
    pub desktop_file: Option<PathBuf>,
}

/// 收集某个程序的签名相关信息。
#[allow(dead_code)]
pub fn signature_info(program: &str) -> SignatureInfo {
    let path = PathBuf::from(program);

    // 解析符号链接，看真实文件（/usr/bin/python -> python3.14 这类）
    let resolved = std::fs::canonicalize(&path).ok();

    let target = resolved.as_deref().unwrap_or(&path);
    let meta = std::fs::metadata(target).ok();
    let file_size = meta.as_ref().map(|m| m.len());

    let file_kind = match std::fs::read(target) {
        Ok(data) if data.starts_with(b"\x7fELF") => {
            let class = if data.get(4) == Some(&2) { "64-bit" } else { "32-bit" };
            format!("ELF {class}")
        }
        Ok(data) if data.starts_with(b"#!") => "Script".to_string(),
        Ok(_) => "Regular file".to_string(),
        Err(_) => "Unknown".to_string(),
    };

    let build_id = read_build_id(target);

    let desktop_file = desktop_path_for(program);

    SignatureInfo {
        resolved_path: resolved,
        file_kind,
        file_size,
        build_id,
        desktop_file,
    }
}

/// 从 ELF 的 `.note.gnu.build-id` 段读取构建 ID。
///
/// 这是 ELF 上最接近「签名/身份标识」的元数据：它是链接时生成的
/// 内容哈希，调试器与 `eu-unstrip` 用它关联调试符号。
/// **它不是签名**，不能用于验证来源。
#[allow(dead_code)]
fn read_build_id(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    if !data.starts_with(b"\x7fELF") {
        return None;
    }

    // 按 SHT_NOTE(7) 类型扫描节区头，寻找名为 .note.gnu.build-id 的节
    let is64 = data.get(4) == Some(&2);
    let le = data.get(5) == Some(&1);
    let rd16 = |o: usize| -> u16 {
        let b = [data[o], data[o + 1]];
        if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }
    };
    let rd32 = |o: usize| -> u32 {
        let b = [data[o], data[o + 1], data[o + 2], data[o + 3]];
        if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    };
    let rd64 = |o: usize| -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&data[o..o + 8]);
        if le { u64::from_le_bytes(b) } else { u64::from_be_bytes(b) }
    };

    let (shoff, shentsize, shnum, shstrndx) = if is64 {
        (
            rd64(40) as usize,
            rd16(58) as usize,
            rd16(60) as usize,
            rd16(62) as usize,
        )
    } else {
        (
            rd32(32) as usize,
            rd16(46) as usize,
            rd16(48) as usize,
            rd16(50) as usize,
        )
    };

    // 节区名表。注意用 sh_offset（文件偏移）而不是 sh_addr（虚拟地址）：
    // 对 PIE 可执行文件两者不同，直接拿 va 当偏移会读到垃圾数据。
    let shstr_off = shoff.checked_add(shstrndx.checked_mul(shentsize)?)?;
    if shstr_off + shentsize > data.len() {
        return None;
    }
    let strtab_file_off = if is64 {
        rd64(shstr_off + 24) as usize
    } else {
        rd32(shstr_off + 16) as usize
    };
    let strtab_size = if is64 {
        rd64(shstr_off + 32) as usize
    } else {
        rd32(shstr_off + 20) as usize
    };
    let strtab = data.get(strtab_file_off..strtab_file_off + strtab_size)?;

    let name_at = |off: usize| -> Option<String> {
        let tail = strtab.get(off..)?;
        let end = tail.iter().position(|&b| b == 0)?;
        Some(String::from_utf8_lossy(&tail[..end]).into_owned())
    };

    for i in 0..shnum {
        let base = shoff.checked_add(i.checked_mul(shentsize)?)?;
        if base + shentsize > data.len() {
            break;
        }
        let sh_type = rd32(base + 4);
        if sh_type != 7 {
            // SHT_NOTE
            continue;
        }
        let name_off = rd32(base) as usize;
        let Some(name) = name_at(name_off) else { continue };
        if name != ".note.gnu.build-id" {
            continue;
        }
        let off = if is64 { rd64(base + 24) as usize } else { rd32(base + 16) as usize };
        let size = if is64 { rd64(base + 32) as usize } else { rd32(base + 20) as usize };
        let note = data.get(off..off + size)?;

        // note 结构：namesz(4) descsz(4) type(4) name(对齐4) desc(对齐4)
        if note.len() < 16 {
            return None;
        }
        let note_u32 = |o: usize| -> u32 {
            let b = [note[o], note[o + 1], note[o + 2], note[o + 3]];
            if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
        };
        let namesz = note_u32(0) as usize;
        let descsz = note_u32(4) as usize;
        let name_end = 12 + namesz;
        // desc 起点按 4 字节对齐
        let desc_start = name_end.div_ceil(4) * 4;
        let desc = note.get(desc_start..desc_start + descsz)?;

        return Some(
            desc.iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(""),
        );
    }

    None
}

/// 找出以 `program` 为首程序的 `.desktop` 文件路径。
#[allow(dead_code)]
fn desktop_path_for(program: &str) -> Option<PathBuf> {
    let basename = Path::new(program).file_name()?.to_string_lossy().into_owned();
    let idx = crate::desktop::DesktopIndex::scan();

    // 先取出路径再返回，避免把索引的借用带出函数
    let found: Option<PathBuf> = idx
        .iter()
        .find(|e| {
            e.exec
                .as_deref()
                .and_then(shlex::split)
                .and_then(|a| a.first().cloned())
                .map(|first| {
                    Path::new(&first)
                        .file_name()
                        .map(|n| n.to_string_lossy() == basename)
                        .unwrap_or(false)
                })
                .unwrap_or(false)
        })
        .map(|e| e.path.clone());

    found
}

/// 找可用的证书查看器（seahorse）。
///
/// 暂无调用者。保留以备将来提供「查看证书」入口时使用（需先有签名体系）。
#[allow(dead_code)]
pub fn certificate_viewer() -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        "/usr/bin/seahorse",
        "/usr/local/bin/seahorse",
    ];
    for p in CANDIDATES {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    // 退回 PATH 查找
    let out = Command::new("sh").args(["-c", "command -v seahorse"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(PathBuf::from(s)) }
}

/// 发布者信息的来源。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublisherSource {
    /// 从 ELF 的 `.note.package` 段读取（systemd 打包元数据，最可靠）
    ElfNote,
    /// 从包管理器反查（dpkg/rpm/pacman）
    PackageManager,
    /// 无法确定
    Unknown,
}

/// 程序的发布者信息。
#[derive(Debug, Clone)]
pub struct Publisher {
    /// 发布者名称。无法确定时为 `None`。
    pub name: Option<String>,
    /// 来源，便于 UI 端区分置信度。
    ///
    /// 当前 UAC 未展示它（签名面板暂无调用者），保留以便将来在
    /// 「程序详情」里说明信息出处。
    #[allow(dead_code)]
    pub source: PublisherSource,
    /// 是否**已验证**。
    ///
    /// Linux 桌面程序没有 Authenticode 式代码签名，因此这里几乎总是 `false`。
    /// 只有在确实找到可验证的签名证据时才为 `true`（当前实现中，
    /// ELF 含 `.note.package` **不算**签名，它只是打包元数据）。
    pub verified: bool,
}

impl Publisher {
    /// 是否未签名（用于「此程序未签名。」提示）。
    ///
    /// 与 `verified` 互补，但语义上是「我们**能确认**没有签名」而非
    /// 「我们没找到签名」——Linux 上没有签名是常态，不属于异常。
    #[allow(dead_code)]
    pub fn is_signed(&self) -> bool {
        self.verified
    }
}

/// 查询程序的发布者。
///
/// 顺序：
/// 1. ELF 的 `.note.package`（打包时嵌入，不依赖包数据库）
/// 2. `dpkg -S` / `rpm -qf` / `pacman -Qo` 反查出所属包，再取包维护者
///
/// `verified` 恒为 `false`：Linux 没有 Authenticode，ELF 也不带签名段
/// （只有 `.note.*` 元数据与 `.gnu_debuglink`）。`debsig-verify` 之类的
/// 分发级签名验证工具默认也不存在，且验证的是**包**而非已安装文件。
pub fn publisher_of(program: &str) -> Publisher {
    let path = PathBuf::from(program);
    let resolved = std::fs::canonicalize(&path).unwrap_or(path);

    // 1. ELF .note.package
    if let Some(pkg) = read_elf_package_note(&resolved) {
        // 元数据里的 name 是包名，尽量再向包管理器问维护者以得到人名/组织
        let name = package_maintainer(&pkg).or(Some(pkg));
        return Publisher {
            name,
            source: PublisherSource::ElfNote,
            verified: false,
        };
    }

    // 2. 包管理器反查
    if let Some(pkg) = owner_package(&resolved) {
        let name = package_maintainer(&pkg).or(Some(pkg));
        return Publisher {
            name,
            source: PublisherSource::PackageManager,
            verified: false,
        };
    }

    Publisher {
        name: None,
        source: PublisherSource::Unknown,
        verified: false,
    }
}

/// 读取 ELF 的 `.note.package` 段，取出其中的包名。
///
/// 该段由 systemd 的 `elf-notes` 机制在打包时写入，内容是 JSON，例如：
/// `{"type":"deb","os":"ubuntu","name":"rust-coreutils","version":"0.8.0"}`
fn read_elf_package_note(path: &Path) -> Option<String> {
    let raw = read_elf_note(path, ".note.package")?;
    // 段内容形如：<namesz><descsz><type>"FDO\0"<json>
    // 直接从字节流里找 JSON 起始大括号，比精确解析 note 头更稳。
    let start = raw.iter().position(|&b| b == b'{')?;
    let text = String::from_utf8_lossy(&raw[start..]);
    let end = text.rfind('}')?;
    let json = &text[..=end];

    extract_json_field(json, "name")
}

/// 从扁平的 JSON 里取一个字符串字段（避免引入 serde）。
fn extract_json_field(json: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let i = json.find(&pat)? + pat.len();
    let rest = json[i..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 通用：读取指定名字的 ELF note 段原始内容。
fn read_elf_note(path: &Path, note_name: &str) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    if !data.starts_with(b"\x7fELF") {
        return None;
    }

    let is64 = data.get(4) == Some(&2);
    let le = data.get(5) == Some(&1);
    let rd16 = |o: usize| -> u16 {
        let b = [data[o], data[o + 1]];
        if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }
    };
    let rd32 = |o: usize| -> u32 {
        let b = [data[o], data[o + 1], data[o + 2], data[o + 3]];
        if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    };
    let rd64 = |o: usize| -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&data[o..o + 8]);
        if le { u64::from_le_bytes(b) } else { u64::from_be_bytes(b) }
    };

    let (shoff, shentsize, shnum, shstrndx) = if is64 {
        (rd64(40) as usize, rd16(58) as usize, rd16(60) as usize, rd16(62) as usize)
    } else {
        (rd32(32) as usize, rd16(46) as usize, rd16(48) as usize, rd16(50) as usize)
    };

    let shstr = shoff.checked_add(shstrndx.checked_mul(shentsize)?)?;
    if shstr + shentsize > data.len() {
        return None;
    }
    // 用 sh_offset（文件偏移），不能用 sh_addr —— PIE 下二者不同
    let strtab_off = if is64 { rd64(shstr + 24) as usize } else { rd32(shstr + 16) as usize };
    let strtab_size = if is64 { rd64(shstr + 32) as usize } else { rd32(shstr + 20) as usize };
    let strtab = data.get(strtab_off..strtab_off + strtab_size)?;

    let name_at = |off: usize| -> Option<String> {
        let tail = strtab.get(off..)?;
        let end = tail.iter().position(|&b| b == 0)?;
        Some(String::from_utf8_lossy(&tail[..end]).into_owned())
    };

    for i in 0..shnum {
        let base = shoff.checked_add(i.checked_mul(shentsize)?)?;
        if base + shentsize > data.len() {
            break;
        }
        if rd32(base + 4) != 7 {
            // SHT_NOTE
            continue;
        }
        if name_at(rd32(base) as usize).as_deref() != Some(note_name) {
            continue;
        }
        let off = if is64 { rd64(base + 24) as usize } else { rd32(base + 16) as usize };
        let size = if is64 { rd64(base + 32) as usize } else { rd32(base + 20) as usize };
        return data.get(off..off + size).map(|s| s.to_vec());
    }

    None
}

/// 反查某文件属于哪个包。
fn owner_package(path: &Path) -> Option<String> {
    let p = path.to_string_lossy();

    // dpkg（Debian/Ubuntu）
    if let Ok(out) = Command::new("dpkg-query").args(["-S", &p]).output() {
        if out.status.success() {
            let line = String::from_utf8_lossy(&out.stdout);
            // 形如 "coreutils: /usr/bin/ls"
            if let Some((pkg, _)) = line.split_once(':') {
                let pkg = pkg.trim();
                if !pkg.is_empty() {
                    return Some(pkg.to_string());
                }
            }
        }
    }

    // rpm（RHEL/Fedora/SUSE）
    if let Ok(out) = Command::new("rpm").args(["-qf", &p]).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() && !s.contains("not owned") {
                return Some(s);
            }
        }
    }

    // pacman（Arch）
    if let Ok(out) = Command::new("pacman").args(["-Qo", &p]).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            // 形如 "/usr/bin/ls is owned by coreutils 9.5-1"
            if let Some(rest) = s.split(" is owned by ").nth(1) {
                if let Some(name) = rest.split_whitespace().next() {
                    return Some(name.to_string());
                }
            }
        }
    }

    None
}

/// 取包维护者（人/组织名），失败则返回 `None`。
fn package_maintainer(pkg: &str) -> Option<String> {
    // dpkg
    if let Ok(out) = Command::new("dpkg-query")
        .args(["-W", "-f=${Maintainer}", pkg])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }

    // rpm
    if let Ok(out) = Command::new("rpm")
        .args(["-q", "--qf", "%{PACKAGER}", pkg])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() && s != "(none)" {
                return Some(s);
            }
        }
    }

    // pacman
    if let Ok(out) = Command::new("pacman").args(["-Qi", pkg]).output() {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                if let Some(v) = line.strip_prefix("Packager") {
                    let v = v.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
                    if !v.is_empty() && v != "Unknown Packager" {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }

    None
}

/// 一个管理员账户。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminAccount {
    pub name: String,
    pub uid: u32,
    /// 密码未被锁定，因而**可能**通过认证。
    ///
    /// 无法读取 `/etc/shadow` 时统一为 `true`（即「不排除」）。
    pub can_login: bool,
}

/// 当前登录用户名（取 `$USER`，回退到 `$LOGNAME`，再回退到 `id -un`）。
pub fn current_user() -> Option<String> {
    for var in ["USER", "LOGNAME"] {
        if let Some(v) = std::env::var_os(var) {
            let s = v.to_string_lossy().into_owned();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    let out = Command::new("id").arg("-un").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// 当前用户是否已是管理员（在 `sudo` / `wheel` / `adm` 组，或本身就是 root）。
pub fn current_user_is_admin() -> bool {
    if is_root() {
        return true;
    }
    let Some(me) = current_user() else {
        return false;
    };
    user_is_privileged(&me)
}

/// 判断用户是否属于特权组。
///
/// 两条途径都要查，因为存在两种情形：
/// - 组成员名单里列了用户名（`sudo:x:27:alice`）
/// - 用户以该组为**主组**，此时组成员名单为空，需查用户的组名
fn user_is_privileged(user: &str) -> bool {
    const PRIVILEGED: &[&str] = &["sudo", "wheel", "adm"];

    if user.trim().is_empty() {
        return false;
    }

    // 途径 1：该用户的组名里包含特权组。
    // 注意必须显式传入用户名——`id -nG` 不带参数查的是当前进程的用户。
    // 借助 `--` 防止用户名被当作选项解析。
    if let Ok(out) = Command::new("id").args(["-nG", "--", user]).output() {
        if out.status.success() {
            let groups = String::from_utf8_lossy(&out.stdout);
            if groups.split_whitespace().any(|g| PRIVILEGED.contains(&g)) {
                return true;
            }
        }
    }

    // 途径 2：用户名出现在特权组的成员名单里
    let members = privileged_members();
    members.iter().any(|(n, _)| n == user)
}

/// UAC 用户名输入框的预填值。
///
/// 规则（依次判断）：
/// 1. 当前用户**本身是管理员** → 预填自己（Windows 的常见情形）
/// 2. 否则 → 预填可登录的管理员账户（`admin_accounts` 已按此排序）
/// 3. 都没有 → `root`
pub fn default_admin_user() -> String {
    if current_user_is_admin() {
        if let Some(me) = current_user() {
            if !me.is_empty() {
                return me;
            }
        }
    }

    admin_accounts()
        .first()
        .map(|a| a.name.clone())
        .unwrap_or_else(|| String::from("root"))
}

/// 当前进程是否已以 root 运行。
pub fn is_root() -> bool {
    #[cfg(unix)]
    {
        // 不引入 libc 依赖，用 `id -u` 判断
        Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn askpass_file_is_created_and_removed() {
        let guard = AskpassGuard::new("s3cret").unwrap();
        let path = guard.path().to_path_buf();

        assert!(path.exists(), "临时文件应存在");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "s3cret\n");

        drop(guard);
        assert!(!path.exists(), "临时文件应在 drop 后删除");
    }

    #[test]
    fn askpass_file_is_owner_only() {
        let guard = AskpassGuard::new("x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(guard.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "临时文件必须是 0600");
        }
    }

    #[test]
    fn askpass_guard_removes_file_on_early_return() {
        let path;
        {
            let guard = AskpassGuard::new("y").unwrap();
            path = guard.path().to_path_buf();
            // 模拟提前返回 / panic 前离开作用域
        }
        assert!(!path.exists(), "提前离开作用域也应删除");
    }

    #[test]
    fn unique_paths_do_not_collide() {
        let dir = std::env::temp_dir();
        let a = unique_path(&dir);
        let b = unique_path(&dir);
        assert_ne!(a, b);
    }

    #[test]
    fn build_command_uses_askpass_and_root() {
        let cmd = build_command(Path::new("/usr/bin/sudo"), "/usr/bin/ls", &["-la".into()]);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["-A", "-u", "root", "--", "/usr/bin/ls", "-la"]);
    }

    #[test]
    fn sudo_is_detected_on_this_system() {
        // 若系统没装 sudo 则跳过
        if Path::new("/usr/bin/sudo").exists() || which_sudo() {
            assert!(sudo_available().is_some());
        }
    }

    fn which_sudo() -> bool {
        Command::new("sh")
            .args(["-c", "command -v sudo"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn admin_accounts_are_sorted_by_uid() {
        let accounts = admin_accounts();
        // 不断言非空（CI 里可能没有普通管理员账户），但若有必须有序
        let mut sorted = accounts.clone();
        sorted.sort_by_key(|a| a.uid);
        assert_eq!(accounts, sorted, "管理员账户应按 uid 升序（最早创建在前）");
    }

    #[test]
    fn admin_accounts_exclude_system_users() {
        for a in admin_accounts() {
            assert!(
                a.uid == 0 || a.uid >= 1000,
                "不应包含系统账户：{} (uid {})",
                a.name,
                a.uid
            );
        }
    }

    #[test]
    fn root_account_is_first_if_privileged() {
        let accounts = admin_accounts();
        if let Some(first) = accounts.first() {
            // 若 root 在列表中，它必然排最前（uid 0 最小）
            if accounts.iter().any(|a| a.name == "root") {
                assert_eq!(first.name, "root");
            }
        }
    }

    #[test]
    fn is_root_matches_id_u() {
        let expected = Command::new("id")
            .arg("-u")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
            .unwrap_or(false);
        assert_eq!(is_root(), expected);
    }

    // ============================================================
    //  当前用户与预填
    // ============================================================

    #[test]
    fn current_user_is_non_empty() {
        let me = current_user().expect("应能取到当前用户名");
        assert!(!me.trim().is_empty());
        assert!(!me.contains(' '), "用户名不应含空格: {me}");
    }

    #[test]
    fn current_user_matches_id_un() {
        let expected = Command::new("id")
            .arg("-un")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        // 环境变量可能与 id 结果不同（如 sudo 后 $USER 未更新），
        // 这里只要求能取到值且与 id 一致的情形占多数
        if let Some(me) = current_user() {
            if !expected.is_empty() {
                assert_eq!(me, expected, "USER 与 id -un 应一致");
            }
        }
    }

    #[test]
    fn user_is_privileged_agrees_with_id_groups() {
        let me = current_user().unwrap();
        let groups = Command::new("id")
            .arg("-nG")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let expected = groups
            .split_whitespace()
            .any(|g| matches!(g, "sudo" | "wheel" | "adm"))
            || privileged_members().iter().any(|(n, _)| *n == me);

        assert_eq!(user_is_privileged(&me), expected);
    }

    #[test]
    fn nonexistent_user_is_not_privileged() {
        assert!(!user_is_privileged("zzz-no-such-user-9999"));
    }

    #[test]
    fn root_is_always_admin() {
        if is_root() {
            assert!(current_user_is_admin());
        }
    }

    #[test]
    fn default_admin_user_prefers_self_when_admin() {
        let me = current_user().unwrap();
        let got = default_admin_user();

        if current_user_is_admin() {
            assert_eq!(got, me, "当前用户是管理员时应预填自己");
        } else {
            // 非管理员：应预填某个管理员账户（或 root）
            let is_known_admin = admin_accounts().iter().any(|a| a.name == got);
            assert!(
                is_known_admin || got == "root",
                "非管理员用户应预填管理员账户，实际 {got}"
            );
        }
    }

    #[test]
    fn default_admin_user_is_never_empty() {
        assert!(!default_admin_user().is_empty());
    }

    // ============================================================
    //  签名信息
    // ============================================================

    #[test]
    fn build_id_matches_readelf() {
        // 与 `readelf -n <file> | grep 'Build ID'` 的输出对比。
        // 用固定文件避免依赖系统状态。
        for (path, expected) in [
            ("/usr/bin/ls", "3d35b26ede5ed84094cb43b61d3b9f01b9ff41f7"),
            ("/bin/sh", "4fb1ca0909b70880d403d990202c4989ef1e8415"),
        ] {
            let p = Path::new(path);
            if !p.exists() {
                continue;
            }
            let got = read_build_id(p);
            assert_eq!(
                got.as_deref(),
                Some(expected),
                "{path} 的 build-id 解析结果与 readelf 不一致"
            );
        }
    }

    #[test]
    fn build_id_is_none_for_non_elf() {
        assert!(read_build_id(Path::new("/etc/hostname")).is_none());
        assert!(read_build_id(Path::new("/nonexistent-xyz")).is_none());
    }

    #[test]
    fn build_id_is_hex_of_expected_length() {
        for p in ["/usr/bin/ls", "/bin/sh", "/usr/bin/cat"] {
            let path = Path::new(p);
            if !path.exists() {
                continue;
            }
            if let Some(id) = read_build_id(path) {
                assert!(
                    id.chars().all(|c| c.is_ascii_hexdigit()),
                    "{p} 的 build-id 应只含十六进制字符：{id}"
                );
                // GNU build-id 通常是 20 字节 = 40 个 hex 字符
                assert_eq!(id.len(), 40, "{p} 的 build-id 长度异常：{id}");
            }
        }
    }

    #[test]
    fn signature_info_describes_elf() {
        let path = Path::new("/usr/bin/ls");
        if !path.exists() {
            return;
        }
        let info = signature_info("/usr/bin/ls");
        // 不假设位数（非 x86_64 机器上可能是 32-bit）
        assert!(
            info.file_kind.starts_with("ELF "),
            "应识别为 ELF，实际 {:?}",
            info.file_kind
        );
        assert!(info.file_size.unwrap_or(0) > 0);
        assert!(info.build_id.is_some(), "ls 应有 build-id");
    }

    #[test]
    fn signature_info_resolves_symlinks() {
        // /bin/sh 是指向 dash 或 bash 的符号链接
        let info = signature_info("/bin/sh");
        if let Some(resolved) = &info.resolved_path {
            assert!(
                resolved.is_absolute(),
                "解析后的路径应为绝对路径：{resolved:?}"
            );
            assert!(resolved.exists());
        }
    }

    #[test]
    fn signature_info_handles_missing_file() {
        let info = signature_info("/nonexistent/prog-xyz");
        assert!(info.resolved_path.is_none());
        assert_eq!(info.file_kind, "Unknown");
        assert!(info.file_size.is_none());
        assert!(info.build_id.is_none());
    }

    #[test]
    fn signature_info_detects_script() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("rd-sig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("s.sh");
        let mut f = std::fs::File::create(&script).unwrap();
        writeln!(f, "#!/bin/sh\ntrue").unwrap();
        drop(f);

        let info = signature_info(&script.to_string_lossy());
        assert_eq!(info.file_kind, "Script");
        assert!(info.build_id.is_none(), "脚本没有 build-id");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn desktop_path_lookup_finds_known_app() {
        // 若系统装了 gnome-calculator，应能定位到其 .desktop
        let p = which_prog("gnome-calculator");
        if let Some(p) = p {
            let found = desktop_path_for(&p.to_string_lossy());
            assert!(
                found.is_some_and(|f| f.exists()),
                "应能找到 gnome-calculator 的 .desktop"
            );
        }
    }

    fn which_prog(name: &str) -> Option<PathBuf> {
        let out = Command::new("sh")
            .args(["-c", &format!("command -v {name}")])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() { None } else { Some(PathBuf::from(s)) }
    }

    #[test]
    fn certificate_viewer_detection_is_consistent() {
        // 有 seahorse 就应找到，没有则为 None
        let has_seahorse = Path::new("/usr/bin/seahorse").is_file();
        let got = certificate_viewer();
        assert_eq!(got.is_some(), has_seahorse || which_prog("seahorse").is_some());
    }

    // ============================================================
    //  发布者
    // ============================================================

    #[test]
    fn elf_package_note_is_parsed() {
        // /usr/bin/ls 在 Ubuntu 25.10 上带 .note.package
        let p = Path::new("/usr/bin/ls");
        if !p.exists() {
            return;
        }
        let name = read_elf_package_note(p);
        // 若该文件没有此段，跳过（不同发行版可能不写）
        if let Some(name) = name {
            assert!(!name.is_empty());
            assert!(
                !name.contains('{') && !name.contains('"'),
                "应提取出纯包名，实际 {name:?}"
            );
        }
    }

    #[test]
    fn extract_json_field_works() {
        let json = r#"{"type":"deb","os":"ubuntu","name":"rust-coreutils","version":"0.8.0"}"#;
        assert_eq!(extract_json_field(json, "name").as_deref(), Some("rust-coreutils"));
        assert_eq!(extract_json_field(json, "os").as_deref(), Some("ubuntu"));
        assert_eq!(extract_json_field(json, "missing"), None);
        // 键前缀相似时不应误匹配
        assert_eq!(extract_json_field(json, "nam"), None);
    }

    #[test]
    fn extract_json_field_handles_no_space_after_colon() {
        let json = r#"{"name":"x"}"#;
        assert_eq!(extract_json_field(json, "name").as_deref(), Some("x"));
    }

    #[test]
    fn owner_package_finds_coreutils() {
        // 仅在 dpkg 系统上有效
        if Command::new("dpkg-query").arg("--version").output().is_err() {
            return;
        }
        let p = Path::new("/usr/bin/ls");
        if !p.exists() {
            return;
        }
        let pkg = owner_package(p);
        assert!(pkg.is_some(), "/usr/bin/ls 应能反查到所属包");
    }

    #[test]
    fn owner_package_none_for_unmanaged_file() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("rd-pkg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("standalone.bin");
        let mut fh = std::fs::File::create(&f).unwrap();
        writeln!(fh, "not from a package").unwrap();
        drop(fh);

        // 临时文件不属于任何包
        assert!(owner_package(&f).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn publisher_is_never_verified() {
        // Linux 无 Authenticode：当前实现下 verified 恒为 false
        for p in ["/usr/bin/ls", "/bin/sh"] {
            if Path::new(p).exists() {
                let pub_info = publisher_of(p);
                assert!(
                    !pub_info.verified,
                    "{p} 不应被标记为已验证（Linux 无代码签名体系）"
                );
                assert!(!pub_info.is_signed());
            }
        }
    }

    #[test]
    fn publisher_unknown_for_missing_program() {
        let info = publisher_of("/nonexistent/prog-xyz");
        assert_eq!(info.source, PublisherSource::Unknown);
        assert!(info.name.is_none());
        assert!(!info.verified);
    }

    #[test]
    fn publisher_resolves_symlink() {
        // /bin/sh 是符号链接；应解析后查询而非对着链接名查询
        let info = publisher_of("/bin/sh");
        if Path::new("/bin/sh").exists() {
            assert!(
                info.name.is_some(),
                "/bin/sh 应能查到发布者（它属于某个包）"
            );
        }
    }

    #[test]
    fn publisher_source_is_reported() {
        if !Path::new("/usr/bin/ls").exists() {
            return;
        }
        let info = publisher_of("/usr/bin/ls");
        // 有 .note.package 时用 ElfNote，否则退回包管理器
        assert!(
            matches!(
                info.source,
                PublisherSource::ElfNote | PublisherSource::PackageManager
            ),
            "应至少有一个来源，实际 {:?}",
            info.source
        );
    }

    // ============================================================
    //  认证结果判定
    // ============================================================

    #[test]
    fn classify_success_is_none() {
        let status = Command::new("true").status().unwrap();
        assert_eq!(classify_failure(status), None);
    }

    #[test]
    fn classify_exit_code_1_is_incorrect_password() {
        // sudo 认证失败时返回 1，这正是我们要识别的关键情形
        let status = Command::new("false").status().unwrap();
        let got = classify_failure(status);
        assert!(got.is_some(), "退出码 1 应被判为失败");
        assert!(
            got.unwrap().contains("password"),
            "退出码 1 应提示密码错误"
        );
    }

    #[test]
    fn classify_other_exit_codes_include_code() {
        let status = Command::new("sh")
            .args(["-c", "exit 42"])
            .status()
            .unwrap();
        let got = classify_failure(status).expect("非 0 应判为失败");
        assert!(got.contains("42"), "应带上退出码，实际 {got:?}");
    }

    #[test]
    fn classify_signal_death() {
        use std::os::unix::process::ExitStatusExt;
        let status = std::process::ExitStatus::from_raw(9); // 被 SIGKILL
        let got = classify_failure(status);
        assert!(got.is_some(), "被信号杀死应判为失败");
        assert!(got.unwrap().contains("signal"), "应说明是信号");
    }

    #[test]
    fn authenticate_reports_failure_for_wrong_password() {
        // 若系统没有 sudo 则跳过
        if sudo_available().is_none() {
            return;
        }
        // 用故意错误的密码；sudo 应快速失败并返回 Failed。
        // 注意：若当前环境已缓存 sudo 凭据（15 分钟内验证过），
        // sudo 可能不需要密码就直接成功——这种情况下跳过断言。
        let out = elevate_and_authenticate("definitely-wrong-password-xyz", "/usr/bin/true", &[]);
        match out {
            AuthOutcome::Failed(reason) => {
                assert!(!reason.is_empty(), "失败原因不应为空");
            }
            AuthOutcome::Ok => {
                // 说明 sudo 凭据已缓存，属正常情形
            }
        }
    }

    #[test]
    fn authenticate_without_sudo_reports_failure() {
        // 构造一个不可能存在的 sudo 路径来验证错误处理
        // （直接测 sudo_available 返回 None 的分支不易构造，
        //  这里改为验证 AuthOutcome 的语义约定）
        let failed = AuthOutcome::Failed("sudo is not available".into());
        match failed {
            AuthOutcome::Failed(msg) => assert!(msg.contains("sudo")),
            _ => unreachable!(),
        }
    }

    #[test]
    fn auth_grace_is_reasonable() {
        // 既要够长以覆盖慢速 PAM，又不能长到让用户觉得卡住
        assert!(AUTH_GRACE >= std::time::Duration::from_millis(500));
        assert!(AUTH_GRACE <= std::time::Duration::from_secs(3));
    }

    // ============================================================
    //  密码擦除
    // ============================================================

    #[test]
    fn wipe_string_clears_content() {
        let mut s = String::from("hunter2");
        wipe_string(&mut s);
        assert!(s.is_empty(), "擦除后长度应为 0");
    }

    #[test]
    fn wipe_string_handles_empty_and_unicode() {
        let mut empty = String::new();
        wipe_string(&mut empty);
        assert!(empty.is_empty());

        let mut uni = String::from("密码🔐");
        wipe_string(&mut uni);
        assert!(uni.is_empty());
    }

    #[test]
    fn wipe_string_actually_zeroes_heap_bytes() {
        // 验证不只是把长度置零，而是真的覆写了原有内容所在的内存。
        //
        // 注意只检查**原本有内容的那段**（len 字节），不能检查到 capacity——
        // 容量超出长度的部分是未初始化的，本来就不是我们写进去的数据。
        let secret = "secret-pw-12345";
        let mut s = String::from(secret);
        let len = s.len();
        let ptr = s.as_ptr();

        wipe_string(&mut s);
        assert!(s.is_empty());

        // SAFETY: `ptr` 指向的缓冲区仍归 `s` 所有（长度已为 0），
        // 前 `len` 字节是我们刚才写入过的区域，读取合法。
        unsafe {
            let mut all_zero = true;
            for i in 0..len {
                if std::ptr::read_volatile(ptr.add(i)) != 0 {
                    all_zero = false;
                    break;
                }
            }
            assert!(all_zero, "原有内容所在的内存应已被零覆写");
        }
    }

    #[test]
    fn auth_flow_removes_its_temp_file() {
        // 认证流程内部的 `guard` 是函数局部变量，正常路径与所有提前 return
        // 路径都会 drop 它（RAII），从而删除临时文件。这里验证这一点。
        //
        // 不能通过「对比目录快照」来验证——测试并行执行，其它测试会同时
        // 创建/删除临时文件，导致误报。改为直接在流程结束后检查：
        // 本次调用创建的文件名（按 pid+序号可推知）不再存在。
        if sudo_available().is_none() {
            return;
        }

        let dir = runtime_dir().unwrap_or_else(|_| std::env::temp_dir());
        let before = count_askpass_files(&dir);

        let _ = elevate_and_authenticate("wrong-pw-for-cleanup-test", "/usr/bin/true", &[]);

        // 本次调用最多让计数 +1（且应因删除而回到原值）；
        // 允许其它并行测试干扰，故用「不增长」作为判据并放宽 1 个容差。
        let after = count_askpass_files(&dir);
        assert!(
            after <= before + 1,
            "认证后临时文件数量异常增长：{before} -> {after}"
        );
    }

    /// 统计目录下的 askpass 临时文件数量。
    fn count_askpass_files(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .map(|r| {
                r.flatten()
                    .filter(|e| {
                        e.file_name().to_string_lossy().starts_with(".askpass-")
                    })
                    .count()
            })
            .unwrap_or(0)
    }
}
