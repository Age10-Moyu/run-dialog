//! 命令查找与启动。
//!
//! 查找顺序（与 Windows「运行」对话框体验对齐，但按 Linux 惯例实现）：
//! 0. Windows URI（`ms-settings:` / `control:`）—— 可选兼容层
//! 1. URL / 协议：`http://` `https://` `mailto:` 等 → `xdg-open`
//! 2. 显式路径：以 `/` `~` `./` `../` 开头，或含 `/`
//!    - 存在且可执行 → 直接执行（按需包终端）
//!    - 存在但不可执行 / 是目录 → `xdg-open`
//! 3. 当前目录（`cwd`）
//! 4. 系统二进制目录
//! 5. `$PATH` 中的目录
//! 6. `.desktop` 应用注册
//! 7. 兜底：`xdg-open` 试一次，仍不行则报「找不到」

use crate::desktop::{DesktopEntry, DesktopIndex, Selection};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 系统二进制目录，按顺序查找。
const SYSTEM_BIN_DIRS: &[&str] = &[
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/local/sbin",
    "/usr/sbin",
    "/sbin",
];

/// 一次成功的启动动作。
#[derive(Debug)]
pub enum Action {
    /// 直接以 argv 启动（可执行文件 / .desktop 解析结果）
    Launch {
        program: String,
        args: Vec<String>,
        tty: bool,
    },
    /// 交给 `xdg-open`（URL、文档、目录）
    Open(String),
    /// 有多个同样好的候选，需要用户选择。
    ///
    /// 仅在设置里开启了「多候选时弹出选择」且候选数 > 1 时出现。
    /// 由 UI 层展示选择框，用户选定后再调 `resolve_pick`。
    Pick { query: String, candidates: Vec<String> },
}

impl Action {
    /// 执行动作。
    ///
    /// 返回 `Err` 表示**启动失败**（程序不存在、没有执行权限、找不到终端等），
    /// 由调用方决定如何提示。子进程自身的运行结果不在此列。
    ///
    /// `Action::Pick` 无法自行执行（需要 UI），直接返回 `Ok(())`，
    /// 调用方应先检查是否为 `Pick` 再决定如何处理。
    pub fn run(&self) -> Result<(), String> {
        match self {
            Action::Open(target) => spawn(Command::new("xdg-open").arg(target)),
            Action::Launch { program, args, tty } => {
                if *tty {
                    run_in_terminal(program, args)
                } else {
                    let mut cmd = Command::new(program);
                    cmd.args(args);
                    spawn(&mut cmd)
                }
            }
            // 选择动作不由自身执行
            Action::Pick { .. } => Ok(()),
        }
    }

    /// 取出选择候选；非 `Pick` 变体返回 `None`。
    pub fn pick_candidates(&self) -> Option<(&str, &[String])> {
        match self {
            Action::Pick { query, candidates } => Some((query, candidates)),
            _ => None,
        }
    }
}

/// 以脱离的标准流启动命令。
fn spawn(cmd: &mut Command) -> Result<(), String> {
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 在终端模拟器里运行程序（argv 形式）。
fn run_in_terminal(program: &str, args: &[String]) -> Result<(), String> {
    let mut command = shell_quote(program);
    for a in args {
        command.push(' ');
        command.push_str(&shell_quote(a));
    }
    run_command_in_terminal(&command)
}

/// 在终端模拟器里运行一整条 shell 命令。
fn run_command_in_terminal(command: &str) -> Result<(), String> {
    let term = find_terminal().ok_or_else(|| "no terminal emulator found".to_string())?;
    spawn(Command::new(term).args(["-e", "sh", "-c", command]))
}

/// 单引号引用，供拼接 shell 命令时使用。
fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=+@".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// 找一个可用的终端模拟器。
fn find_terminal() -> Option<&'static str> {
    const CANDIDATES: &[&str] = &[
        "x-terminal-emulator",
        "ptyxis",
        "gnome-terminal",
        "konsole",
        "xfce4-terminal",
        "alacritty",
        "kitty",
        "foot",
        "xterm",
    ];
    CANDIDATES
        .iter()
        .copied()
        .find(|p| which_in_path(p).is_some())
}

/// 在 `$PATH` 中查找可执行文件。
pub fn which_in_path(prog: &str) -> Option<PathBuf> {
    if prog.contains('/') {
        let p = PathBuf::from(prog);
        return if is_executable_file(&p) { Some(p) } else { None };
    }
    for dir in path_dirs() {
        let candidate = dir.join(prog);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// `$PATH` 拆分结果。
///
/// POSIX 规定空项表示当前目录；这里**跳过**空项，避免意外执行 cwd 下的程序。
fn path_dirs() -> Vec<PathBuf> {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// 是否为可执行文件（存在、是普通文件、有执行位）。
pub fn is_executable_file(p: &Path) -> bool {
    let Ok(md) = std::fs::metadata(p) else {
        return false;
    };
    if !md.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        md.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// 展开 `~` 与 `~user`。
pub fn expand_tilde(s: &str) -> String {
    if s == "~" {
        if let Some(h) = std::env::var_os("HOME") {
            return h.to_string_lossy().into_owned();
        }
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(h) = std::env::var_os("HOME") {
            return Path::new(&h).join(rest).to_string_lossy().into_owned();
        }
    }
    s.to_string()
}

/// 判断输入是否「看起来像显式路径」。
fn looks_like_path(s: &str) -> bool {
    s.starts_with('/')
        || s.starts_with('~')
        || s.starts_with("./")
        || s.starts_with("../")
        || s.contains('/')
}

/// 判断输入是否像 URL / 带协议的 URI。
///
/// 仅接受**已知协议**：`a:b` 这种模糊形式（如 `C:\...`）不当作 URI，
/// 否则 Windows 路径会被误判。
fn looks_like_uri(s: &str) -> bool {
    const SCHEMES: &[&str] = &[
        "http", "https", "ftp", "ftps", "sftp", "ssh", "telnet", "mailto", "news", "nntp",
        "file", "smb", "nfs", "magnet", "irc", "ircs", "xmpp", "tel", "callto", "geo",
        "webcal", "help", "man", "info", "ghelp",
    ];
    let Some((scheme, _)) = s.split_once(':') else {
        return false;
    };
    if scheme.is_empty() || !scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if !scheme
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return false;
    }
    SCHEMES.contains(&scheme.to_ascii_lowercase().as_str())
}

/// 一次查找的完整上下文。
pub struct Resolver {
    pub index: DesktopIndex,
    /// 对话框启动时的当前目录
    pub cwd: PathBuf,
    /// 选中项（当前恒为空，预留给将来的文件关联）
    pub selection: Selection,
    /// 多个 `.desktop` 同样匹配时，是否返回 `Action::Pick` 让用户选。
    ///
    /// 关闭时取第一个匹配（默认，行为可预测）。
    pub pick_ambiguous: bool,
}

impl Resolver {
    pub fn new() -> Self {
        Self::with_pick(crate::config::Config::load().pick_desktop)
    }

    /// 以指定选择策略构造。
    pub fn with_pick(pick_ambiguous: bool) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"))
        });
        Self {
            index: DesktopIndex::scan(),
            cwd,
            selection: Selection::default(),
            pick_ambiguous,
        }
    }

    /// 判断某个可执行文件是否需要终端窗口。
    ///
    /// 两条客观依据，任一成立即可：
    /// 1. 某个 `.desktop` 以它为首程序且声明 `Terminal=true`（如 `vim.desktop`）
    /// 2. 它本身是 shebang 指向交互式解释器的脚本
    fn tty_for(&self, program: &Path) -> bool {
        desktop_wants_terminal(&self.index, program) || needs_terminal_for_path(program)
    }

    /// 按 7 步顺序解析输入。返回 `None` 表示找不到。
    pub fn resolve(&self, input: &str) -> Option<Action> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }

        // ---- 1. URL / 协议 ----
        if looks_like_uri(input) {
            return Some(Action::Open(input.to_string()));
        }

        // 拆出首个 token 用于查找（其余作为参数）
        let (head, rest) = split_head(input);

        // ---- 2. 显式路径 ----
        if looks_like_path(head) {
            let expanded = expand_tilde(head);
            let exists = Path::new(&expanded).exists();
            if exists {
                let path = Path::new(&expanded);
                if is_executable_file(path) {
                    let args = split_args(rest);
                    let tty = needs_terminal_for_path(path);
                    return Some(Action::Launch {
                        program: expanded,
                        tty,
                        args,
                    });
                }
                // 目录 / 普通文件 → 交给 xdg-open
                return Some(Action::Open(expanded));
            }
            // 路径不存在：继续往下试（可能是个名字里带 / 的命令），
            // 但最终若找不到会报「找不到」
        }

        // ---- 3. 当前目录 ----
        if !head.contains('/') {
            let candidate = self.cwd.join(head);
            if is_executable_file(&candidate) {
                let tty = self.tty_for(&candidate);
                return Some(Action::Launch {
                    program: candidate.to_string_lossy().into_owned(),
                    args: split_args(rest),
                    tty,
                });
            }
        }

        // ---- 4. 系统二进制目录 ----
        if !head.contains('/') {
            for dir in SYSTEM_BIN_DIRS {
                let candidate = Path::new(dir).join(head);
                if is_executable_file(&candidate) {
                    let tty = self.tty_for(&candidate);
                    return Some(Action::Launch {
                        program: candidate.to_string_lossy().into_owned(),
                        args: split_args(rest),
                        tty,
                    });
                }
            }
        }

        // ---- 5. $PATH ----
        if let Some(p) = which_in_path(head) {
            // 参数按 shlex 规则拆分后逐个传给 argv，**不经过 shell**。
            // 因此通配符、管道、变量展开不会被解释——这是刻意的：
            // 运行框只负责「按名字启动程序」，不做一个隐式 shell。
            let tty = self.tty_for(&p);
            return Some(Action::Launch {
                program: p.to_string_lossy().into_owned(),
                args: split_args(rest),
                tty,
            });
        }

        // ---- 6. .desktop 应用注册 ----
        //
        // 注意用**完整输入**而非首 token：应用名常含空格（"Text Editor"、
        // "Visual Studio Code"）。用首 token 查会把 "Text Editor" 拆成
        // "Text"，从而误命中 Keywords 里含单个 "Text" 的 gvim/vim。
        //
        // `Idle` 是 KDE 的占位条目，只表示「会话空闲时用」，不可作为候选。
        let usable: Vec<&DesktopEntry> = self
            .index
            .find_all(input)
            .into_iter()
            .filter(|e| try_exec_ok(e) && e.build_launch(&self.selection).is_some())
            .collect();

        // 多个候选：按设置决定是弹选择还是取第一个
        if usable.len() > 1 && self.pick_ambiguous {
            return Some(Action::Pick {
                query: input.to_string(),
                candidates: usable.iter().map(|e| e.name.clone()).collect(),
            });
        }

        if let Some(entry) = usable.first() {
            if let Some(launch) = entry.build_launch(&self.selection) {
                return Some(Action::Launch {
                    program: launch.program,
                    args: launch.args,
                    tty: entry.terminal,
                });
            }
        }

        // ---- 7. 兜底：当作文件名/URL 让 xdg-open 试一次 ----
        if Path::new(input).exists() {
            return Some(Action::Open(input.to_string()));
        }

        None
    }

    /// 执行用户在 `Action::Pick` 里选中的候选（按**显示名**匹配）。
    ///
    /// 再次走 `.desktop` 分支，避免把候选索引跨越 UI 调用传递。
    pub fn resolve_pick(&self, query: &str, chosen_name: &str) -> Option<Action> {
        let entry = self
            .index
            .find_all(query)
            .into_iter()
            .filter(|e| try_exec_ok(e))
            .find(|e| e.name == chosen_name)?;

        let launch = entry.build_launch(&self.selection)?;
        Some(Action::Launch {
            program: launch.program,
            args: launch.args,
            tty: entry.terminal,
        })
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

/// 校验 `.desktop` 的 `TryExec`：指向的程序必须存在，否则该条目不可用。
fn try_exec_ok(entry: &DesktopEntry) -> bool {
    match entry.try_exec.as_deref() {
        None => true,
        Some(te) if te.is_empty() => true,
        Some(te) => which_in_path(te).is_some() || Path::new(te).exists(),
    }
}

/// 该可执行文件是否被某个 `.desktop` 声明为 `Terminal=true`。
///
/// 通过 `Exec=` 的首个 token 的 basename 匹配。这样在第 5 步命中
/// `$PATH` 的程序（如 `vim`）也能沿用 `.desktop` 的终端设置。
fn desktop_wants_terminal(index: &DesktopIndex, program: &Path) -> bool {
    let Some(base) = program.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    index
        .iter()
        .any(|e| e.terminal && exec_program_is(e, base))
}

/// 某条目的 `Exec=` 是否以指定程序名开头。
fn exec_program_is(entry: &DesktopEntry, basename: &str) -> bool {
    let Some(exec) = entry.exec.as_deref() else {
        return false;
    };
    let Some(argv) = shlex::split(exec) else {
        return false;
    };
    argv.first()
        .and_then(|p| Path::new(p).file_name())
        .and_then(|s| s.to_str())
        .is_some_and(|n| n == basename)
}

/// 拆分输入的首个 token 与其余部分（此处仅用于「名字 + 参数」形式）。
///
/// 这里刻意保持简单：真正的参数解析交给 shell（`Action::Shell`）或
/// `.desktop` 的 `shlex`，避免自己实现一套不完整的引号规则。
fn split_head(input: &str) -> (&str, &str) {
    match input.find(char::is_whitespace) {
        Some(i) => (&input[..i], input[i..].trim_start()),
        None => (input, ""),
    }
}

/// 把参数字符串按空白拆成参数列表。
fn split_args(rest: &str) -> Vec<String> {
    if rest.trim().is_empty() {
        return Vec::new();
    }
    shlex::split(rest).unwrap_or_else(|| {
        rest.split_whitespace().map(|s| s.to_string()).collect()
    })
}

/// 交互式解释器名单。
///
/// 命中意味着**裸跑该程序会进入 REPL / 交互式会话**，因而需要 tty。
/// 同时用于两处：
/// - 脚本的 shebang 解释器（`#!/usr/bin/python3`）
/// - 直接输入的程序名（`python3` 本身是 ELF，但裸跑仍进 REPL）
///
/// 名字按 basename 比对，`python3.12` 这类版本后缀会被归一化掉。
const INTERACTIVE_INTERPRETERS: &[&str] = &[
    "sh", "bash", "zsh", "ksh", "mksh", "dash", "ash", "csh", "tcsh", "fish",
    "python", "python2", "python3", "pypy", "pypy3", "ipython", "ipython3",
    "perl", "perl5", "ruby", "irb", "node", "nodejs", "deno", "bun",
    "php", "lua", "luajit", "tclsh", "wish", "guile", "racket", "scheme",
    "julia", "R", "octave", "gnuplot", "gdb", "lldb", "sqlite3", "psql",
    "mysql", "redis-cli", "mongo", "mongosh", "bc", "expect", "awk",
    "gawk", "mawk", "elixir", "iex", "erl", "ghci", "cabal", "sbcl", "clisp",
];

/// 判断程序名是否为交互式解释器。
///
/// 会剥离版本号后缀：`python3.12` → `python3`、`perl5.38` → `perl5`。
fn is_interactive_interpreter(program: &str) -> bool {
    if INTERACTIVE_INTERPRETERS.contains(&program) {
        return true;
    }
    // 剥掉 `3.12` / `5.38.2` 这类纯数字与点组成的后缀
    let base = program
        .trim_end_matches(|c: char| c.is_ascii_digit() || c == '.')
        .trim_end_matches('.');
    !base.is_empty() && INTERACTIVE_INTERPRETERS.contains(&base)
}

/// 判断某个可执行文件是否需要在终端中运行。
///
/// 依据（任一成立即可）：
/// - `.desktop` 的 `Terminal=true` —— 在 `Resolver::tty_for` 里处理，最可靠
/// - **脚本**且 shebang 指向交互式解释器
/// - **ELF 但程序名是交互式解释器**（`python3`、`node` 等裸跑进 REPL）
///
/// 刻意**不**因为「ELF 未链接 GUI 库」就包终端：那会把 `ls`、`grep`
/// 这类一次性输出的核心工具也包起来。直接启动它们只会一闪而过，
/// 与 Windows 上 `dir` 的表现一致，没有必要为它维护一份工具黑名单。
pub fn needs_terminal_for_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    let Ok(head) = read_head(path, 512) else {
        // 读不到内容（如权限不足）：退化为按名字判断
        return is_interactive_interpreter(&name);
    };

    // ELF：只有名字是解释器时才包终端
    if head.starts_with(b"\x7fELF") {
        return is_interactive_interpreter(&name);
    }

    // 脚本：看 shebang 的解释器
    if head.starts_with(b"#!") {
        let first_line = head.split(|&b| b == b'\n').next().unwrap_or(&head);
        let line = String::from_utf8_lossy(first_line);
        // `#!/usr/bin/env python3` 这类要取 env 后面的真实解释器
        let mut parts = line.trim_start_matches("#!").split_whitespace();
        let mut interp = parts.next().unwrap_or("");
        if interp.ends_with("/env") || interp == "env" {
            interp = parts.next().unwrap_or("");
        }
        let interp = interp.rsplit('/').next().unwrap_or("");
        return is_interactive_interpreter(interp);
    }

    false
}

/// 读取文件开头若干字节。
fn read_head(path: &Path, n: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; n];
    let mut read = 0;
    while read < n {
        match f.read(&mut buf[read..]) {
            Ok(0) => break,
            Ok(k) => read += k,
            Err(e) => return Err(e),
        }
    }
    buf.truncate(read);
    Ok(buf)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_detection_accepts_known_schemes() {
        assert!(looks_like_uri("http://x"));
        assert!(looks_like_uri("https://example.com/a"));
        assert!(looks_like_uri("mailto:a@b.c"));
        assert!(looks_like_uri("ftp://x"));
    }

    #[test]
    fn uri_detection_rejects_windows_paths() {
        // C:\ 不是 URI，否则 win_compat 会被绕过
        assert!(!looks_like_uri(r"C:\Windows"));
        assert!(!looks_like_uri(r"C:/Windows"));
        assert!(!looks_like_uri("not-a-uri"));
        assert!(!looks_like_uri("a b:c"));
    }

    #[test]
    fn uri_detection_rejects_arbitrary_scheme() {
        // 未注册协议不该被当作 URL
        assert!(!looks_like_uri("foobar://x"));
    }

    #[test]
    fn path_detection() {
        assert!(looks_like_path("/usr/bin/ls"));
        assert!(looks_like_path("~/doc"));
        assert!(looks_like_path("./x"));
        assert!(looks_like_path("../x"));
        assert!(looks_like_path("dir/file"));
        assert!(!looks_like_path("ls"));
        assert!(!looks_like_path("gnome-calculator"));
    }

    #[test]
    fn tilde_expansion() {
        let saved = std::env::var("HOME").ok();
        std::env::set_var("HOME", "/home/tester");
        assert_eq!(expand_tilde("~/a"), "/home/tester/a");
        assert_eq!(expand_tilde("~"), "/home/tester");
        assert_eq!(expand_tilde("/abs"), "/abs");
        // ~user 不展开（需要 passwd 查询，这里保持原样）
        assert_eq!(expand_tilde("~other/a"), "~other/a");
        match saved {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn split_head_separates_first_token() {
        assert_eq!(split_head("gedit foo.txt"), ("gedit", "foo.txt"));
        assert_eq!(split_head("gedit"), ("gedit", ""));
    }

    #[test]
    fn split_args_respects_quotes() {
        assert_eq!(split_args("\"a b\" c"), ["a b", "c"]);
        assert!(split_args("").is_empty());
    }

    #[test]
    fn executable_detection() {
        assert!(is_executable_file(Path::new("/bin/sh")));
        assert!(!is_executable_file(Path::new("/etc/hostname")));
        assert!(!is_executable_file(Path::new("/nonexistent-xyz")));
        // 目录不算
        assert!(!is_executable_file(Path::new("/usr")));
    }

    #[test]
    fn which_finds_common_programs() {
        assert!(which_in_path("sh").is_some());
        assert!(which_in_path("ls").is_some());
        assert!(which_in_path("definitely-not-a-program-xyz").is_none());
    }

    #[test]
    fn which_accepts_absolute_path() {
        assert!(which_in_path("/bin/sh").is_some());
        assert!(which_in_path("/bin/definitely-missing").is_none());
    }

    #[test]
    fn terminal_needed_for_shell_scripts() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("rd-term-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let write = |name: &str, content: &str| {
            let p = dir.join(name);
            let mut f = std::fs::File::create(&p).unwrap();
            writeln!(f, "{content}").unwrap();
            p
        };

        // 交互式解释器 → 需要终端（直接运行相当于进 REPL）
        assert!(needs_terminal_for_path(&write("a.sh", "#!/bin/bash\necho hi")));
        assert!(needs_terminal_for_path(&write("b.py", "#!/usr/bin/python3")));

        // `#!/usr/bin/env <interp>` 要取 env 后面的真实解释器
        assert!(needs_terminal_for_path(&write("c.py", "#!/usr/bin/env python3")));

        // 非交互解释器 → 不需要终端
        assert!(!needs_terminal_for_path(&write("d.sh", "#!/usr/bin/nonexistent-interp")));
        // 没有 shebang 的文件不算
        assert!(!needs_terminal_for_path(&write("e.txt", "plain text")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn elf_binaries_do_not_get_terminal() {
        // 刻意不为非 GUI 的 ELF 包终端：ls 这类工具一闪而过是可接受的，
        // 否则就要维护一份永远不全的工具黑名单。
        for p in ["/bin/sh", "/usr/bin/ls", "/bin/ls"] {
            let path = Path::new(p);
            if path.exists() {
                // /bin/sh 是解释器（dash/bash 的链接），属预期例外
                let expect = path
                    .file_name()
                    .is_some_and(|n| n == "sh");
                assert_eq!(
                    needs_terminal_for_path(path),
                    expect,
                    "{p} 的终端判定不符合预期"
                );
            }
        }
        // 纯 CLI 工具一律不包终端
        for p in ["/usr/bin/ls", "/usr/bin/cat", "/usr/bin/grep"] {
            let path = Path::new(p);
            if path.exists() {
                assert!(!needs_terminal_for_path(path), "{p} 不该包终端");
            }
        }
    }

    #[test]
    fn repl_interpreters_get_terminal_even_as_elf() {
        // python3 本体是 ELF（非脚本），但裸跑进 REPL，需要 tty
        for p in ["/usr/bin/python3", "/usr/bin/python", "/usr/bin/node"] {
            let path = Path::new(p);
            if path.exists() {
                assert!(
                    needs_terminal_for_path(path),
                    "{p} 是交互式解释器，应包终端"
                );
            }
        }
    }

    #[test]
    fn interpreter_name_normalization() {
        // 带版本号的解释器名要能归一化
        assert!(is_interactive_interpreter("python3.12"));
        assert!(is_interactive_interpreter("python3"));
        assert!(is_interactive_interpreter("perl5.38"));
        assert!(is_interactive_interpreter("bash"));
        assert!(is_interactive_interpreter("node"));

        // 非解释器不受影响
        assert!(!is_interactive_interpreter("ls"));
        assert!(!is_interactive_interpreter("python3-config"));
        assert!(!is_interactive_interpreter(""));
        assert!(!is_interactive_interpreter("."));
    }

    #[test]
    fn resolve_python_gets_terminal() {
        if which_in_path("python3").is_none() {
            return;
        }
        let r = Resolver::new();
        match r.resolve("python3") {
            Some(Action::Launch { tty, .. }) => {
                assert!(tty, "python3 应在终端中启动");
            }
            other => panic!("python3 应能解析，实际 {other:?}"),
        }
    }

    #[test]
    fn resolve_opens_urls() {
        let r = Resolver::new();
        match r.resolve("https://example.com") {
            Some(Action::Open(u)) => assert_eq!(u, "https://example.com"),
            other => panic!("应识别为 URL，实际 {other:?}"),
        }
    }

    #[test]
    fn resolve_executes_known_binary() {
        let r = Resolver::new();
        match r.resolve("/bin/sh") {
            Some(Action::Launch { program, .. }) => assert_eq!(program, "/bin/sh"),
            other => panic!("应直接执行，实际 {other:?}"),
        }
    }

    #[test]
    fn resolve_opens_existing_directory() {
        let r = Resolver::new();
        match r.resolve("/usr") {
            Some(Action::Open(p)) => assert_eq!(p, "/usr"),
            other => panic!("目录应交给 xdg-open，实际 {other:?}"),
        }
    }

    #[test]
    fn resolve_finds_binary_in_path() {
        let r = Resolver::new();
        assert!(r.resolve("ls").is_some(), "PATH 中的 ls 应能找到");
    }

    #[test]
    fn resolve_returns_none_for_garbage() {
        let r = Resolver::new();
        assert!(r.resolve("zzz-does-not-exist-9999").is_none());
        assert!(r.resolve("").is_none());
    }

    /// 回归：含空格的应用名必须整体匹配。
    ///
    /// 曾经用首 token 查 .desktop，导致 "Text Editor" 被拆成 "Text"，
    /// 误命中 Keywords 里含单个 "Text" 的 gvim。
    #[test]
    fn multiword_app_name_is_not_truncated_to_first_token() {
        let r = Resolver::new();
        if let Some(entry) = r.index.find("Text Editor") {
            // 索引里若有该应用，解析结果必须指向它，而不是 gvim/vim
            let resolved = r.resolve("Text Editor").expect("应能解析");
            if let Action::Launch { program, .. } = resolved {
                assert_ne!(
                    program, "gvim",
                    "\"Text Editor\" 不应被拆成 \"Text\" 而误命中 gvim"
                );
                let _ = entry;
            }
        }
    }

    #[test]
    fn try_exec_gate_rejects_missing_program() {
        let mut e = DesktopEntry {
            name: "X".into(),
            exec: Some("x".into()),
            ..Default::default()
        };
        assert!(try_exec_ok(&e), "无 TryExec 时应放行");

        e.try_exec = Some("zzz-definitely-missing-9999".into());
        assert!(!try_exec_ok(&e), "TryExec 不存在时应拦下");

        e.try_exec = Some("sh".into());
        assert!(try_exec_ok(&e), "TryExec 存在时应放行");
    }

    #[test]
    fn desktop_wants_terminal_matches_by_exec_basename() {
        let mut e = DesktopEntry {
            name: "Vim".into(),
            exec: Some("vim %F".into()),
            terminal: true,
            ..Default::default()
        };
        let idx = DesktopIndex::from_entries(vec![e.clone()]);
        assert!(desktop_wants_terminal(&idx, Path::new("/usr/bin/vim")));
        assert!(!desktop_wants_terminal(&idx, Path::new("/usr/bin/ls")));

        // Terminal=false 时不要求终端
        e.terminal = false;
        let idx2 = DesktopIndex::from_entries(vec![e]);
        assert!(!desktop_wants_terminal(&idx2, Path::new("/usr/bin/vim")));
    }

    #[test]
    fn exec_program_matching_handles_unparseable_exec() {
        let mut e = DesktopEntry {
            name: "X".into(),
            exec: Some("unbalanced \"quote".into()),
            ..Default::default()
        };
        assert!(!exec_program_is(&e, "x"));
        e.exec = None;
        assert!(!exec_program_is(&e, "x"));
    }

    /// 真实系统抽查：`vim` 若有 .desktop 声明 Terminal=true，应包终端。
    #[test]
    fn real_vim_gets_terminal_if_desktop_says_so() {
        let r = Resolver::new();
        let vim = which_in_path("vim");
        let Some(vim) = vim else { return };

        let wants = desktop_wants_terminal(&r.index, &vim);
        if let Some(Action::Launch { tty, .. }) = r.resolve("vim") {
            assert_eq!(tty, wants, "resolve 的 tty 应与 .desktop 声明一致");
        }
    }

    // ============================================================
    //  参数传递：一律 argv，不经过 shell
    // ============================================================

    #[test]
    fn arguments_are_passed_as_argv_not_shell() {
        let r = Resolver::new();
        match r.resolve("ls -la") {
            Some(Action::Launch { program, args, .. }) => {
                assert!(program.ends_with("ls"));
                assert_eq!(args, ["-la"]);
            }
            other => panic!("应走 Launch(argv)，实际 {other:?}"),
        }
    }

    #[test]
    fn shell_metacharacters_are_not_interpreted() {
        let r = Resolver::new();

        // 通配符保持字面量，不做 glob 展开
        match r.resolve("ls *.txt") {
            Some(Action::Launch { args, .. }) => assert_eq!(args, ["*.txt"]),
            other => panic!("实际 {other:?}"),
        }

        // 管道作为普通参数传给程序，不会变成 shell 管道
        match r.resolve("ls | wc") {
            Some(Action::Launch { args, .. }) => assert_eq!(args, ["|", "wc"]),
            other => panic!("实际 {other:?}"),
        }

        // 重定向同理
        match r.resolve("ls > /tmp/x") {
            Some(Action::Launch { args, .. }) => assert_eq!(args, [">", "/tmp/x"]),
            other => panic!("实际 {other:?}"),
        }

        // 变量不展开
        match r.resolve("echo $HOME") {
            Some(Action::Launch { args, .. }) => assert_eq!(args, ["$HOME"]),
            other => panic!("实际 {other:?}"),
        }
    }

    #[test]
    fn quoted_arguments_are_grouped() {
        let r = Resolver::new();
        match r.resolve("ls \"a b\" c") {
            Some(Action::Launch { args, .. }) => assert_eq!(args, ["a b", "c"]),
            other => panic!("实际 {other:?}"),
        }
    }

    #[test]
    fn run_reports_failure_for_missing_program() {
        let action = Action::Launch {
            program: "/nonexistent/prog-xyz".into(),
            args: vec![],
            tty: false,
        };
        assert!(action.run().is_err(), "启动不存在的程序应返回 Err");
    }

    // ============================================================
    //  多候选选择
    // ============================================================

    #[test]
    fn pick_only_when_multiple_candidates() {
        // "Text" 命中多个（Keywords 含 Text），开启选择时应返回 Pick
        let r = Resolver::with_pick(true);
        match r.resolve("Text") {
            Some(Action::Pick { query, candidates }) => {
                assert_eq!(query, "Text");
                assert!(candidates.len() > 1, "应有多个候选: {candidates:?}");
            }
            other => panic!("多候选且已开启选择时应返回 Pick，实际 {other:?}"),
        }
    }

    #[test]
    fn no_pick_when_disabled() {
        // 关闭开关时取第一个，行为可预测
        let r = Resolver::with_pick(false);
        match r.resolve("Text") {
            Some(Action::Launch { .. }) => {}
            other => panic!("关闭选择时应直接 Launch，实际 {other:?}"),
        }
    }

    #[test]
    fn unique_match_never_picks() {
        // 只有一个候选时即使开启选择也直接启动
        let r = Resolver::with_pick(true);
        for q in ["Calculator", "Clocks"] {
            if r.index.find_all(q).is_empty() {
                continue;
            }
            match r.resolve(q) {
                Some(Action::Launch { .. }) => {}
                other => panic!("{q} 唯一匹配时应直接 Launch，实际 {other:?}"),
            }
        }
    }

    #[test]
    fn pick_candidates_accessor() {
        let pick = Action::Pick {
            query: "q".into(),
            candidates: vec!["A".into(), "B".into()],
        };
        assert_eq!(pick.pick_candidates(), Some(("q", &["A".into(), "B".into()][..])));

        let launch = Action::Launch {
            program: "x".into(),
            args: vec![],
            tty: false,
        };
        assert!(launch.pick_candidates().is_none());

        // Pick 自行 run 时空操作（需 UI 处理）
        assert!(pick.run().is_ok());
    }

    #[test]
    fn resolve_pick_launches_chosen() {
        let r = Resolver::with_pick(true);
        if let Some(Action::Pick { query, candidates }) = r.resolve("Text") {
            let chosen = &candidates[0];
            match r.resolve_pick(&query, chosen) {
                Some(Action::Launch { .. }) => {}
                other => panic!("选定 {chosen} 后应能启动，实际 {other:?}"),
            }
            // 不存在的名字返回 None
            assert!(r.resolve_pick(&query, "no-such-app").is_none());
        }
    }
}
