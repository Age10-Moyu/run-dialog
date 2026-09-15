//! `.desktop` 条目的解析与 Exec 字段码展开。
//!
//! 遵循 XDG Desktop Entry Specification：
//! <https://specifications.freedesktop.org/desktop-entry-spec/latest/exec-variables.html>
//!
//! 设计原则（「空可以，错不行」）：
//! - 字段码是 `.desktop` 互操作契约的一部分，**展开为空不等于丢弃**。
//!   当前启动器没有「选中文件」，`%f`/`%U` 展开为空是正常语义；
//!   将来接入文件关联时这条链路不用重写。
//! - 未知字段码按规范是**致命错误**：该 `Exec` 行不可执行，返回 `None`。
//! - 顺序：**先 `shlex::split` 拆 token，再逐 token 展开**。反过来会
//!   让 `%c` 的值里带空格时把参数拆碎。

use shlex::split as shlex_split;
use std::path::{Path, PathBuf};

/// 启动 .desktop 时传入的「选中项」。
///
/// 当前启动器恒为 `Selection::default()`（空），但类型保留完整语义，
/// 将来接入 `xdg-open` 的文件关联时直接填充即可。
#[derive(Default, Debug, Clone)]
pub struct Selection {
    /// 选中的文件（对应 `%f` / `%F`）。URL 会被过滤掉。
    pub files: Vec<String>,
    /// 选中的 URL（对应 `%u` / `%U`）。
    pub urls: Vec<String>,
}

/// 展开后的启动命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
}

/// 一个 `.desktop` 条目中我们关心的字段。
#[derive(Debug, Clone, Default)]
pub struct DesktopEntry {
    /// `Name` 的当前语言取值。
    pub name: String,
    /// `Icon` 取值。
    pub icon: Option<String>,
    /// `Exec` 原始行。
    pub exec: Option<String>,
    /// `Terminal=true` 时需要终端窗口。
    pub terminal: bool,
    /// `TryExec` 指定的可执行文件名（用于校验条目是否可用）。
    pub try_exec: Option<String>,
    /// `MimeType`，分号分隔。
    ///
    /// 当前查找流程未使用（启动器没有「选中文件」），但按规范保留：
    /// 将来接入输入文件路径 → MIME 查询 → 关联应用的链路时需要它。
    #[allow(dead_code)]
    pub mime_types: Vec<String>,
    /// `Keywords`，分号分隔。
    pub keywords: Vec<String>,
    /// `.desktop` 文件路径（供 `%k` 使用）。
    pub path: PathBuf,
}

impl DesktopEntry {
    /// 按规范展开 `Exec`，得到最终 argv。
    ///
    /// 返回 `None` 表示该条目不可用（`Exec` 缺失、引号不配对、含未知字段码）。
    pub fn build_launch(&self, sel: &Selection) -> Option<Launch> {
        let exec = self.exec.as_deref()?;
        let argv = expand_exec(exec, self, sel)?;
        let (program, args) = argv.split_first()?;
        Some(Launch {
            program: program.clone(),
            args: args.to_vec(),
        })
    }
}

/// 按规范展开 `Exec=` 行。
///
/// 步骤：`shlex` 拆 token → 逐 token 展开字段码 → 展平。
/// 任一 token 含未知字段码时整体返回 `None`（规范要求该行不得执行）。
pub fn expand_exec(exec: &str, entry: &DesktopEntry, sel: &Selection) -> Option<Vec<String>> {
    let tokens = shlex_split(exec)?;

    let mut out: Vec<String> = Vec::with_capacity(tokens.len() + 4);
    for token in tokens {
        expand_token(&token, entry, sel, &mut out)?;
    }

    if out.is_empty() {
        return None;
    }
    Some(out)
}

/// 展开单个 token，把结果追加到 `out`。
///
/// 一个 token 可能展开成 0 个、1 个或多个参数：
/// - `%i` → `["--icon", 值]`（两个参数，且必须是独立 token）
/// - `%f`（无选中文件）→ 零个参数
/// - `%c` → 与同一 token 内的字面量**按出现顺序**拼接成一个参数
fn expand_token(
    token: &str,
    entry: &DesktopEntry,
    sel: &Selection,
    out: &mut Vec<String>,
) -> Option<()> {
    // 快速路径：不含 `%` 的 token 原样保留
    if !token.contains('%') {
        out.push(token.to_string());
        return Some(());
    }

    // 当前正在累积的参数；None 表示尚未开始累积。
    // 字段码与字面量按扫描顺序依次追加，保证 "echo %c" 得到 "echo My App"。
    let mut current: Option<String> = None;

    // 结束当前参数：仅在非空时落盘，避免 %c 展开为空时留下空参数
    fn flush(current: &mut Option<String>, out: &mut Vec<String>) {
        if let Some(s) = current.take() {
            if !s.is_empty() {
                out.push(s);
            }
        }
    }

    // 把一段字面量追加到当前参数
    fn push_literal(current: &mut Option<String>, s: &str) {
        current.get_or_insert_with(String::new).push_str(s);
    }

    let mut chars = token.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        if c != '%' {
            push_literal(&mut current, &c.to_string());
            continue;
        }

        let Some(&(_, code)) = chars.peek() else {
            // 结尾孤立的 `%`：按规范属于非法
            return None;
        };
        chars.next();

        match code {
            // ---- 展开为「一个参数」：与前后字面量拼接 ----
            'c' => {
                if !entry.name.is_empty() {
                    push_literal(&mut current, &entry.name);
                }
            }
            'k' => {
                push_literal(&mut current, &entry.path.to_string_lossy());
            }
            // ---- 展开为「零个或一个参数」 ----
            'f' | 'u' => {
                let first = if code == 'f' {
                    sel.files.first()
                } else {
                    sel.urls.first()
                };
                if let Some(v) = first {
                    push_literal(&mut current, v);
                }
            }
            // ---- 展开为「零个或多个参数」 ----
            // 混合在字面量中时退化为拼接；独立 token 时展开为多个参数。
            'F' | 'U' => {
                let list = if code == 'F' { &sel.files } else { &sel.urls };
                if list.is_empty() {
                    // 展开为空：不产生参数，字面量继续累积
                } else if current.as_ref().is_some_and(|s| !s.is_empty()) {
                    // 已累积字面量：整体作为一个参数
                    for v in list {
                        push_literal(&mut current, v);
                    }
                    flush(&mut current, out);
                } else {
                    flush(&mut current, out);
                    for v in list {
                        out.push(v.clone());
                    }
                }
            }
            // ---- 展开为「两个参数」，必须是独立 token ----
            'i' => {
                if let Some(icon) = entry.icon.as_deref().filter(|s| !s.is_empty()) {
                    if current.is_some() {
                        // 规范要求 %i 独立成 token；混合时保守判为不可用
                        return None;
                    }
                    out.push("--icon".to_string());
                    out.push(icon.to_string());
                }
            }
            // ---- 废弃码：按规范移除该 token ----
            // 规范原文：deprecated field codes %d %D %n %N %v %m should be
            // removed from the executable arguments.（即整条 token 作废）
            'd' | 'D' | 'n' | 'N' | 'v' | 'm' => {
                return Some(());
            }
            // ---- 字面量百分号 ----
            '%' => push_literal(&mut current, "%"),
            // ---- 未知字段码：规范要求该 Exec 行不可执行 ----
            _ => return None,
        }

        let _ = i;
    }

    flush(&mut current, out);
    Some(())
}

// ============================================================
//  .desktop 文件解析
// ============================================================

/// 扫描并索引 `.desktop` 条目。
#[derive(Debug, Default)]
pub struct DesktopIndex {
    entries: Vec<DesktopEntry>,
}

/// `.desktop` 搜索路径，按优先级从高到低（用户目录在前，可覆盖系统条目）。
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // XDG_DATA_HOME（默认 ~/.local/share）
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
        });
    if let Some(h) = data_home {
        dirs.push(h.join("applications"));
    }

    // XDG_DATA_DIRS（默认 /usr/local/share:/usr/share）
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for d in data_dirs.split(':').filter(|s| !s.is_empty()) {
        dirs.push(PathBuf::from(d).join("applications"));
    }

    // Flatpak 导出目录（系统级 + 用户级）
    dirs.push(PathBuf::from(
        "/var/lib/flatpak/exports/share/applications",
    ));
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/flatpak/exports/share/applications"));
    }

    // Snap 导出目录
    dirs.push(PathBuf::from("/var/lib/snapd/desktop/applications"));

    dirs.retain(|d| d.is_dir());
    dirs
}

impl DesktopIndex {
    /// 扫描所有搜索目录，构建索引。
    ///
    /// 解析失败、`NoDisplay=true`、`Hidden=true` 的条目会被忽略——
    /// UI 层不报解析错误，静默跳过即可。
    pub fn scan() -> Self {
        let mut entries = Vec::new();
        let mut seen: Vec<String> = Vec::new();

        for dir in search_dirs() {
            let Ok(read) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut files: Vec<PathBuf> = read
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("desktop"))
                .collect();
            files.sort();

            for path in files {
                // 按规范，靠前目录里的同名条目优先
                let id = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if seen.contains(&id) {
                    continue;
                }
                if let Some(entry) = parse_desktop_file(&path) {
                    seen.push(id);
                    entries.push(entry);
                }
            }
        }

        DesktopIndex { entries }
    }

    /// 索引是否为空（供测试与将来的诊断使用）。
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 遍历所有条目。
    pub fn iter(&self) -> impl Iterator<Item = &DesktopEntry> {
        self.entries.iter()
    }

    /// 用给定条目直接构造索引（供测试使用，跳过文件系统扫描）。
    #[cfg(test)]
    pub fn from_entries(entries: Vec<DesktopEntry>) -> Self {
        DesktopIndex { entries }
    }

    /// 按应用名 / 关键字查找，返回**最佳匹配级别的所有候选**。
    ///
    /// 匹配分四级，由精确到宽松，取第一个非空级别：
    /// 1. `Name` 完全匹配（忽略大小写）
    /// 2. `Keywords` 完全匹配
    /// 3. `Name` 包含
    /// 4. `Keywords` 包含
    ///
    /// 分级而非混排，可以避免 `Text` 这类通用词压过真正的应用名。
    /// 同一级别可能命中多个（如 `Name=Files` 与 `Keywords=files`），
    /// 交由调用方决定是取第一个还是让用户选。
    pub fn find_all(&self, query: &str) -> Vec<&DesktopEntry> {
        let q = query.trim();
        if q.is_empty() {
            return Vec::new();
        }

        let eq_ci = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        let contains_ci =
            |hay: &str, needle: &str| hay.to_lowercase().contains(&needle.to_lowercase());

        // 1. Name 完全匹配
        let hit: Vec<&DesktopEntry> = self
            .entries
            .iter()
            .filter(|e| !e.name.is_empty() && eq_ci(&e.name, q))
            .collect();
        if !hit.is_empty() {
            return hit;
        }

        // 2. Keywords 完全匹配
        let hit: Vec<&DesktopEntry> = self
            .entries
            .iter()
            .filter(|e| !e.name.is_empty() && e.keywords.iter().any(|k| eq_ci(k, q)))
            .collect();
        if !hit.is_empty() {
            return hit;
        }

        // 3. Name 包含
        let hit: Vec<&DesktopEntry> = self
            .entries
            .iter()
            .filter(|e| !e.name.is_empty() && contains_ci(&e.name, q))
            .collect();
        if !hit.is_empty() {
            return hit;
        }

        // 4. Keywords 包含
        self.entries
            .iter()
            .filter(|e| !e.name.is_empty() && e.keywords.iter().any(|k| contains_ci(k, q)))
            .collect()
    }

    /// 取最佳匹配的第一个条目。
    ///
    /// 生产路径改用 `find_all`（需要判断候选数量），这个便捷方法主要供测试使用。
    #[allow(dead_code)]
    pub fn find(&self, query: &str) -> Option<&DesktopEntry> {
        self.find_all(query).into_iter().next()
    }
}

/// 解析单个 `.desktop` 文件。
fn parse_desktop_file(path: &Path) -> Option<DesktopEntry> {
    let content = std::fs::read_to_string(path).ok()?;

    let mut in_main = false;
    let mut name: Option<String> = None;
    let mut name_fallback: Option<String> = None;
    let mut icon: Option<String> = None;
    let mut exec: Option<String> = None;
    let mut terminal = false;
    let mut try_exec: Option<String> = None;
    let mut mime_types: Vec<String> = Vec::new();
    let mut keywords: Vec<String> = Vec::new();
    let mut keywords_fallback: Vec<String> = Vec::new();
    let mut no_display = false;
    let mut hidden = false;
    let mut entry_type: Option<String> = None;

    // 当前语言环境，用于挑选 Name[xx] / Keywords[xx]
    let langs = current_langs();

    for line in content.lines() {
        let line = line.trim();

        if line.starts_with('[') && line.ends_with(']') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        // 带语言标记的键，如 Name[zh_CN]
        if let Some((base, lang)) = key.split_once('[') {
            let lang = lang.trim_end_matches(']');
            if langs.iter().any(|l| l.eq_ignore_ascii_case(lang)) {
                match base {
                    "Name" => name = Some(value.to_string()),
                    "Keywords" => keywords = split_list(value),
                    _ => {}
                }
            }
            // 记录无语言标记的兜底值
            if base == "Name" && name_fallback.is_none() {
                name_fallback = Some(value.to_string());
            }
            continue;
        }

        match key {
            "Name" => {
                if name_fallback.is_none() {
                    name_fallback = Some(value.to_string());
                }
            }
            "Icon" => icon = Some(value.to_string()),
            "Exec" => exec = Some(value.to_string()),
            "Terminal" => terminal = value.eq_ignore_ascii_case("true"),
            "TryExec" => try_exec = Some(value.to_string()),
            "MimeType" => mime_types = split_list(value),
            "Keywords" => {
                if keywords_fallback.is_empty() {
                    keywords_fallback = split_list(value);
                }
            }
            "Type" => entry_type = Some(value.to_string()),
            "NoDisplay" => no_display = value.eq_ignore_ascii_case("true"),
            "Hidden" => hidden = value.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }

    // 只保留 Application 类型
    if let Some(t) = entry_type.as_deref() {
        if !t.eq_ignore_ascii_case("Application") {
            return None;
        }
    }
    if no_display || hidden {
        return None;
    }

    let name = name.or(name_fallback).unwrap_or_default();
    if keywords.is_empty() {
        keywords = keywords_fallback;
    }

    Some(DesktopEntry {
        name,
        icon,
        exec,
        terminal,
        try_exec,
        mime_types,
        keywords,
        path: path.to_path_buf(),
    })
}

/// 当前语言候选列表，用于挑选 `Name[xx]`。
///
/// 形如 `["zh_CN", "zh", "C"]`，按优先级排序。
fn current_langs() -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    if let Ok(l) = std::env::var("LANGUAGE") {
        raw.extend(l.split(':').map(|s| s.to_string()));
    }
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                raw.push(v);
                break;
            }
        }
    }

    let mut out = Vec::new();
    for r in raw {
        let base = r.split('.').next().unwrap_or(&r).split('@').next().unwrap_or(&r);
        if base.is_empty() || base == "C" || base == "POSIX" {
            continue;
        }
        if !out.iter().any(|x: &String| x == base) {
            out.push(base.to_string());
        }
        if let Some(i) = base.find('_') {
            let short = &base[..i];
            if !out.iter().any(|x| x == short) {
                out.push(short.to_string());
            }
        }
    }
    out
}

/// 拆分分号分隔的列表值（`MimeType`、`Keywords`）。
fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(exec: &str, name: &str, icon: Option<&str>) -> DesktopEntry {
        DesktopEntry {
            name: name.to_string(),
            icon: icon.map(|s| s.to_string()),
            exec: Some(exec.to_string()),
            terminal: false,
            try_exec: None,
            mime_types: Vec::new(),
            keywords: Vec::new(),
            path: PathBuf::from("/usr/share/applications/test.desktop"),
        }
    }

    fn argv(exec: &str, e: &DesktopEntry, sel: &Selection) -> Option<Vec<String>> {
        expand_exec(exec, e, sel)
    }

    #[test]
    fn plain_exec_without_field_codes() {
        let e = entry("gedit", "Editor", None);
        assert_eq!(argv("gedit", &e, &Selection::default()).unwrap(), ["gedit"]);
    }

    #[test]
    fn quoted_args_survive_split() {
        let e = entry("foo \"a b\" c", "App", None);
        assert_eq!(
            argv("foo \"a b\" c", &e, &Selection::default()).unwrap(),
            ["foo", "a b", "c"]
        );
    }

    // ---- 规范的核心场景：没有选中文件时展开为空，而不是丢弃 ----

    #[test]
    fn percent_u_expands_to_nothing_without_selection() {
        let e = entry("firefox %U", "Firefox", None);
        assert_eq!(
            argv("firefox %U", &e, &Selection::default()).unwrap(),
            ["firefox"]
        );
    }

    #[test]
    fn percent_f_expands_to_nothing_without_selection() {
        let e = entry("okular %f", "Okular", None);
        assert_eq!(
            argv("okular %f", &e, &Selection::default()).unwrap(),
            ["okular"]
        );
    }

    #[test]
    fn percent_u_uses_selection_when_present() {
        let e = entry("firefox %U", "Firefox", None);
        let sel = Selection {
            files: vec![],
            urls: vec!["https://example.com".into()],
        };
        assert_eq!(
            argv("firefox %U", &e, &sel).unwrap(),
            ["firefox", "https://example.com"]
        );
    }

    #[test]
    fn percent_f_uses_first_file_only() {
        let e = entry("okular %f", "Okular", None);
        let sel = Selection {
            files: vec!["/a.pdf".into(), "/b.pdf".into()],
            urls: vec![],
        };
        assert_eq!(argv("okular %f", &e, &sel).unwrap(), ["okular", "/a.pdf"]);
    }

    #[test]
    fn percent_upper_f_passes_all_files() {
        let e = entry("app %F", "App", None);
        let sel = Selection {
            files: vec!["/a.pdf".into(), "/b.pdf".into()],
            urls: vec![],
        };
        assert_eq!(
            argv("app %F", &e, &sel).unwrap(),
            ["app", "/a.pdf", "/b.pdf"]
        );
    }

    // ---- 顺序：先拆 token 再展开 ----

    #[test]
    fn percent_c_inside_quotes_stays_one_argument() {
        // 规范文档里的例子：Exec=sh -c "echo %c" %i
        let e = entry("sh -c \"echo %c\" %i", "My App", Some("myicon"));
        assert_eq!(
            argv("sh -c \"echo %c\" %i", &e, &Selection::default()).unwrap(),
            ["sh", "-c", "echo My App", "--icon", "myicon"]
        );
    }

    #[test]
    fn percent_c_without_quotes_keeps_token_intact() {
        let e = entry("app --label=%c", "My App", None);
        // 值里有空格也不会把参数拆碎
        assert_eq!(
            argv("app --label=%c", &e, &Selection::default()).unwrap(),
            ["app", "--label=My App"]
        );
    }

    #[test]
    fn percent_i_expands_to_two_arguments() {
        let e = entry("app %i %U", "App", Some("icon1"));
        assert_eq!(
            argv("app %i %U", &e, &Selection::default()).unwrap(),
            ["app", "--icon", "icon1"]
        );
    }

    #[test]
    fn percent_i_empty_icon_expands_to_nothing() {
        let e = entry("app %i", "App", None);
        assert_eq!(argv("app %i", &e, &Selection::default()).unwrap(), ["app"]);
        let e2 = entry("app %i", "App", Some(""));
        assert_eq!(argv("app %i", &e2, &Selection::default()).unwrap(), ["app"]);
    }

    #[test]
    fn percent_i_mixed_with_literal_is_rejected() {
        // %i 必须是独立 token，混合时保守判为不可用
        let e = entry("app --prefix%i", "App", Some("icon1"));
        assert!(argv("app --prefix%i", &e, &Selection::default()).is_none());
    }

    #[test]
    fn percent_k_gives_desktop_path() {
        let e = entry("app %k", "App", None);
        assert_eq!(
            argv("app %k", &e, &Selection::default()).unwrap(),
            ["app", "/usr/share/applications/test.desktop"]
        );
    }

    #[test]
    fn empty_name_expands_to_nothing() {
        let e = entry("app \"%c\"", "", None);
        // Name 为空 → 展开为空，不留空参数
        assert_eq!(argv("app \"%c\"", &e, &Selection::default()).unwrap(), ["app"]);
    }

    // ---- 未知字段码是致命错误 ----

    #[test]
    fn unknown_field_code_rejects_entry() {
        let e = entry("app %z", "App", None);
        assert!(argv("app %z", &e, &Selection::default()).is_none());
    }

    #[test]
    fn trailing_percent_rejects_entry() {
        let e = entry("app %", "App", None);
        assert!(argv("app %", &e, &Selection::default()).is_none());
    }

    // ---- 废弃码移除 token ----

    #[test]
    fn deprecated_codes_remove_token() {
        for code in ["%d", "%D", "%n", "%N", "%v", "%m"] {
            let exec = format!("app {code}");
            let e = entry(&exec, "App", None);
            assert_eq!(
                argv(&exec, &e, &Selection::default()).unwrap(),
                ["app"],
                "废弃码 {code} 应移除该 token"
            );
        }
    }

    #[test]
    fn deprecated_code_removes_whole_token_not_just_code() {
        // 规范说「从可执行参数中移除」，即整条参数，而非只剥掉 %d
        let e = entry("app --old=%d", "App", None);
        assert_eq!(
            argv("app --old=%d", &e, &Selection::default()).unwrap(),
            ["app"]
        );
    }

    // ---- 字面量百分号 ----

    #[test]
    fn double_percent_is_literal_percent() {
        let e = entry("app 100%%", "App", None);
        assert_eq!(
            argv("app 100%%", &e, &Selection::default()).unwrap(),
            ["app", "100%"]
        );
    }

    #[test]
    fn unbalanced_quotes_reject_entry() {
        let e = entry("app \"unterminated", "App", None);
        assert!(argv("app \"unterminated", &e, &Selection::default()).is_none());
    }

    #[test]
    fn empty_exec_rejects_entry() {
        let e = entry("   ", "App", None);
        assert!(argv("   ", &e, &Selection::default()).is_none());
    }

    #[test]
    fn build_launch_splits_program_and_args() {
        let e = entry("gedit --new-window %U", "Editor", None);
        let l = e.build_launch(&Selection::default()).unwrap();
        assert_eq!(l.program, "gedit");
        assert_eq!(l.args, ["--new-window"]);
    }

    #[test]
    fn missing_exec_returns_none() {
        let mut e = entry("x", "App", None);
        e.exec = None;
        assert!(e.build_launch(&Selection::default()).is_none());
    }

    // ============================================================
    //  .desktop 文件解析
    // ============================================================

    /// 把内容写成临时 .desktop 文件后解析。
    ///
    /// 用进程内自增序号保证文件名唯一：测试并行执行，固定文件名会互相覆盖。
    fn parse_str(content: &str) -> Option<DesktopEntry> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);

        let dir = std::env::temp_dir().join(format!(
            "rd-desktop-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("probe.desktop");
        std::fs::write(&path, content).unwrap();
        parse_desktop_file(&path)
    }

    #[test]
    fn parses_basic_fields() {
        let e = parse_str(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Text Editor\n\
             Exec=gedit %U\n\
             Icon=gedit\n\
             Terminal=false\n\
             Keywords=editor;text;\n\
             MimeType=text/plain;\n",
        )
        .unwrap();

        assert_eq!(e.name, "Text Editor");
        assert_eq!(e.exec.as_deref(), Some("gedit %U"));
        assert_eq!(e.icon.as_deref(), Some("gedit"));
        assert!(!e.terminal);
        assert_eq!(e.keywords, ["editor", "text"]);
        assert_eq!(e.mime_types, ["text/plain"]);
    }

    #[test]
    fn skips_non_application_types() {
        let e = parse_str("[Desktop Entry]\nType=Link\nName=X\nURL=https://x\n");
        assert!(e.is_none());
    }

    #[test]
    fn skips_nodisplay_and_hidden() {
        assert!(parse_str("[Desktop Entry]\nType=Application\nName=X\nNoDisplay=true\n").is_none());
        assert!(parse_str("[Desktop Entry]\nType=Application\nName=X\nHidden=true\n").is_none());
    }

    #[test]
    fn ignores_other_groups() {
        // [Desktop Action ...] 里的键不应覆盖主组
        let e = parse_str(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Main\n\
             Exec=main\n\
             [Desktop Action New]\n\
             Name=New Window\n\
             Exec=main --new\n",
        )
        .unwrap();
        assert_eq!(e.name, "Main");
        assert_eq!(e.exec.as_deref(), Some("main"));
    }

    #[test]
    fn terminal_flag_parsed_case_insensitively() {
        let e = parse_str("[Desktop Entry]\nType=Application\nName=X\nExec=x\nTerminal=True\n").unwrap();
        assert!(e.terminal);
    }

    #[test]
    fn splits_semicolon_lists_and_drops_empties() {
        assert_eq!(split_list("a;b;;c;"), ["a", "b", "c"]);
        assert_eq!(split_list(""), Vec::<String>::new());
    }

    #[test]
    fn tryexec_and_mimetype_captured() {
        let e = parse_str(
            "[Desktop Entry]\nType=Application\nName=X\nExec=x %f\nTryExec=x\nMimeType=image/png;\n",
        )
        .unwrap();
        assert_eq!(e.try_exec.as_deref(), Some("x"));
        assert_eq!(e.mime_types, ["image/png"]);
    }

    #[test]
    fn full_pipeline_exec_with_selection() {
        // 模拟将来接入文件关联：%f 拿到真实路径
        let e = parse_str("[Desktop Entry]\nType=Application\nName=Okular\nExec=okular %f\n").unwrap();
        let sel = Selection {
            files: vec!["/home/me/doc.pdf".into()],
            urls: vec![],
        };
        let l = e.build_launch(&sel).unwrap();
        assert_eq!(l.program, "okular");
        assert_eq!(l.args, ["/home/me/doc.pdf"]);
    }

    // ============================================================
    //  查找精度
    // ============================================================

    fn index_of(entries: Vec<DesktopEntry>) -> DesktopIndex {
        DesktopIndex { entries }
    }

    fn mk(name: &str, keywords: &[&str]) -> DesktopEntry {
        DesktopEntry {
            name: name.to_string(),
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            exec: Some("x".into()),
            ..Default::default()
        }
    }

    #[test]
    fn exact_name_wins_over_keyword_match() {
        // gvim 的 Keywords 含 "Text"/"editor"，GNOME 编辑器的 Name 是 "Text Editor"。
        // 查询 "Text Editor" 必须命中后者。
        let idx = index_of(vec![
            mk("GVim", &["Text", "editor"]),
            mk("Text Editor", &["write", "notepad"]),
        ]);
        assert_eq!(idx.find("Text Editor").unwrap().name, "Text Editor");
    }

    #[test]
    fn exact_keyword_match_before_partial_name() {
        let idx = index_of(vec![
            mk("Text Editor Thing", &[]),
            mk("Other", &["Foo"]),
        ]);
        // "Foo" 精确命中 Keywords，优先于 "Text Editor Thing" 的部分包含
        assert_eq!(idx.find("Foo").unwrap().name, "Other");
    }

    #[test]
    fn partial_name_match_works() {
        let idx = index_of(vec![mk("Text Editor", &[])]);
        assert_eq!(idx.find("text editor").unwrap().name, "Text Editor");
        assert_eq!(idx.find("Text Edit").unwrap().name, "Text Editor");
    }

    #[test]
    fn keyword_substring_does_not_overreach() {
        // 查询 "Text Editor" 不应命中 Keywords 只有单个 "Text" 的条目
        let idx = index_of(vec![mk("GVim", &["Text"]), mk("Real", &["Text Editor"])]);
        assert_eq!(idx.find("Text Editor").unwrap().name, "Real");
    }

    #[test]
    fn query_longer_than_keyword_is_not_a_keyword_hit() {
        let idx = index_of(vec![mk("GVim", &["Text", "editor"])]);
        // "Text Editor" 不等于任一 Keywords，且 Name 不含它
        assert!(idx.find("Text Editor").is_none());
    }

    // ============================================================
    //  候选列表（find_all）
    // ============================================================

    #[test]
    fn find_all_returns_every_entry_at_best_level() {
        // 同级别的多个命中全部返回
        let idx = index_of(vec![
            mk("Vim", &["Text"]),
            mk("GVim", &["Text"]),
            mk("Other", &[]),
        ]);
        let hits = idx.find_all("Text");
        assert_eq!(hits.len(), 2, "同级别命中应全部返回: {hits:?}");
    }

    #[test]
    fn find_all_stops_at_exact_level() {
        // 有 Name 精确匹配时，不返回粗级别匹配
        let idx = index_of(vec![
            mk("Text", &[]),
            mk("Text Editor", &[]),
            mk("Other", &["Text"]),
        ]);
        let hits = idx.find_all("Text");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Text");
    }

    #[test]
    fn find_all_empty_for_blank_query() {
        let idx = index_of(vec![mk("A", &[])]);
        assert!(idx.find_all("").is_empty());
        assert!(idx.find_all("   ").is_empty());
    }

    #[test]
    fn find_all_skips_unnamed_entries() {
        let idx = index_of(vec![mk("", &["Text"]), mk("Real", &["Text"])]);
        let hits = idx.find_all("Text");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Real");
    }

    #[test]
    fn find_equals_first_of_find_all() {
        let idx = index_of(vec![mk("A", &["q"]), mk("B", &["q"])]);
        let all = idx.find_all("q");
        assert_eq!(idx.find("q").unwrap().name, all[0].name);
    }

    /// 对真实系统 .desktop 的抽查：多词应用名应精确匹配到对应条目。
    ///
    /// 只在相应应用确实安装时断言，避免测试依赖具体发行版。
    #[test]
    fn real_system_lookup_prefers_exact_name() {
        let idx = DesktopIndex::scan();

        // "Text Editor" 若同时存在 Name=Text Editor 与 Keywords 含 "Text" 的条目，
        // 必须命中前者
        if let Some(hit) = idx.find("Text Editor") {
            assert_eq!(
                hit.name, "Text Editor",
                "多词查询不应被 Keywords 中的单词抢先命中"
            );
        }

        // 索引不应包含 NoDisplay/Hidden 条目
        for e in idx.iter() {
            assert!(!e.name.is_empty(), "入索引的条目必须有 Name: {:?}", e.path);
        }
    }

    /// 真实文件抽查：`vim.desktop` 声明了 `Terminal=true`。
    ///
    /// 该文件含大量 `Name[xx]=` 本地化行，用于验证本地化键不会干扰主键解析。
    #[test]
    fn real_vim_desktop_parses_terminal_flag() {
        let p = Path::new("/usr/share/applications/vim.desktop");
        if !p.exists() {
            return;
        }
        let e = parse_desktop_file(p).expect("vim.desktop 应可解析");
        assert_eq!(e.name, "Vim", "Name 应取自主键而非本地化键");
        assert!(e.terminal, "vim.desktop 声明了 Terminal=true");
        assert_eq!(e.try_exec.as_deref(), Some("vim"));
        assert!(e.exec.as_deref().is_some_and(|x| x.starts_with("vim")));
    }
}
