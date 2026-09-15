//! 国际化（i18n）：基于 gettext 的薄封装。
//!
//! 约定：
//! - **msgid 一律用英文**（也就是「开发语言」），例如 `t("Run")`、`tf("{1}...")`。
//!   源码里不出现任何中文，避免语序、歧义、非 ASCII 键等问题。
//! - 译文放在 `po/<lang>.po`，由 gettext 工具链编译成 `.mo`。
//! - 界面文案统一走 `t()` / `tf()`，不要直接写字面量。
//!
//! 目录约定（`.mo` 安装在 `<prefix>/share/locale/<lang>/LC_MESSAGES/run-dialog.mo`），
//! 按顺序探测：
//! 1. `$RUN_DIALOG_LOCALEDIR` —— 调试用，直接指向含 `locale/` 的目录
//! 2. `$XDG_DATA_HOME`（默认 `~/.local/share`）—— `build` 脚本装到这里的 `locale/`
//! 3. 可执行文件旁的 `locale/` —— 开发期直接运行 `target/<profile>/run-dialog`
//! 4. 可执行文件旁 `../share/locale` —— 安装后 `<prefix>/bin` + `<prefix>/share`
//! 5. `/usr/share/locale`（macOS 为 `/usr/local/share/locale`）
//!
//! 用法：
//! ```ignore
//! use crate::i18n::{t, tf, init};
//!
//! init();                          // main() 里最先调用
//! let title = t("Run");            // 普通文案
//! let msg = tf("{1} not found", &[&input]); // 命名占位符
//! ```

use gettextrs::{bindtextdomain, setlocale, textdomain, LocaleCategory};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// gettext 域，同时也是 `.mo` 文件名（`run-dialog.mo`）。
pub const TEXTDOMAIN: &str = "run-dialog";

/// 判断某本地化根目录下是否装有本域的 `.mo`。
///
/// 只要当前语言（或语言的基名，如 `zh_CN` → `zh`）有译文就算命中。
/// 仅检查 `is_dir()` 是不够的：`~/.local/share/locale` 往往存在但里面
/// 只有别的程序的译文，那样会让我们过早停止探测。
fn has_catalog(locale_root: &Path) -> bool {
    // gettext 依次尝试完整 locale、去修饰、截断到 '_' 之前的基名
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(lang) = std::env::var("LANGUAGE") {
        candidates.extend(lang.split(':').map(|s| s.to_string()));
    }
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                candidates.push(v);
                break;
            }
        }
    }

    for c in candidates {
        // 去掉 .UTF-8 / .utf8 之类的编码后缀
        let base = c.split('.').next().unwrap_or(&c);
        let names: Vec<&str> = if let Some(i) = base.find('_') {
            vec![base, &base[..i]]
        } else {
            vec![base]
        };
        for name in names {
            if locale_root
                .join(name)
                .join("LC_MESSAGES")
                .join(format!("{TEXTDOMAIN}.mo"))
                .is_file()
            {
                return true;
            }
        }
    }
    false
}

/// 取本地化目录（含 `locale/` 的那一层）。结果缓存。
fn locale_dir() -> Option<&'static Path> {
    static CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            // 收集候选根目录，按优先级排列
            let mut candidates: Vec<PathBuf> = Vec::new();

            // 1. 环境变量显式指定（只做 is_dir 检查，便于调试时强制生效）
            if let Some(dir) = std::env::var_os("RUN_DIALOG_LOCALEDIR") {
                let p = PathBuf::from(dir).join("locale");
                if p.is_dir() {
                    return Some(p);
                }
            }

            // 2. XDG_DATA_HOME / ~/.local/share —— `build` 脚本或 `make install` 的默认位置
            if let Some(base) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
            }) {
                candidates.push(base.join("locale"));
            }

            // 3. 可执行文件旁：开发期 target/<profile>/locale、安装后 <prefix>/share/locale
            if let Ok(exe) = std::env::current_exe() {
                if let Some(dir) = exe.parent() {
                    candidates.push(dir.join("locale"));
                    if let Some(prefix) = dir.parent() {
                        candidates.push(prefix.join("share/locale"));
                    }
                }
            }

            // 4. 系统路径
            candidates.push(if cfg!(target_os = "macos") {
                PathBuf::from("/usr/local/share/locale")
            } else {
                PathBuf::from("/usr/share/locale")
            });

            candidates.into_iter().find(|p| has_catalog(p))
        })
        .as_deref()
}

/// 初始化国际化环境。应在 `main()` 一开始、创建 GTK 界面之前调用。
///
/// 未安装任何 `.mo` 也不会报错，此时所有 `t()` 返回 msgid（英文）。
pub fn init() {
    // 按当前环境（LANG / LC_MESSAGES / LC_ALL）生效。
    //
    // SAFETY: setlocale 会修改进程级全局状态，本身不是线程安全的。这里在
    // main() 最开始调用，此时还没有创建任何线程（GTK 也尚未初始化），
    // 因此不存在并发访问；传空串表示「从环境变量读取」，不会解引用裸指针。
    let _ = unsafe { setlocale(LocaleCategory::LcAll, "") };

    if let Some(dir) = locale_dir() {
        let _ = bindtextdomain(TEXTDOMAIN, dir);
    }
    let _ = textdomain(TEXTDOMAIN);
}

/// 取文案。`msgid` 为英文原文；无对应翻译时返回 `msgid` 本身。
pub fn t(msgid: &str) -> String {
    gettextrs::gettext(msgid)
}

/// 标记「稍后才翻译」的字符串，仅供 `xgettext` 提取。
///
/// `xgettext` 只能识别字面量**直接**传给 `t()` 的写法。文案若先存进数组、
/// 之后才通过变量传给 `t()`，它就提取不到：
///
/// ```ignore
/// t("Literal")                            // ✅ 能提取
/// let rows = [("Literal", "Subtitle")];   // ❌ 提取不到
/// ....title(&t(title))
/// ```
///
/// 用 `N_()` 包住数组中的字符串，配合 `xgettext --keyword=N_` 即可收集：
///
/// ```ignore
/// let rows = [(N_("Literal"), N_("Subtitle"))];
/// ....title(&t(title))   // 运行时照常翻译
/// ```
///
/// 它展开为**原字符串本身**，不产生翻译行为 —— 运行时翻译仍由 `t()` 完成。
/// 若误把它当 `t()` 使用，界面会显示英文原文。
///
/// 用函数而非宏，是因为 `xgettext --keyword=N_` 识别的是函数调用写法
/// `N_("...")`；宏需要写成 `N_!("...")`，提取不到。
/// `non_snake_case` 是本函数名的有意例外。
#[allow(non_snake_case)]
pub const fn N_(msgid: &'static str) -> &'static str {
    msgid
}

/// 取带占位符的文案。
///
/// 占位符使用 gettext 的**位置记号** `{1}` `{2}`…，译文可自由调整语序或重复引用
/// 同一参数——这是它相对顺序填充 `{}` 的关键优势。
///
/// ```ignore
/// tf("{1} 找不到文件“{2}”。", &[distro, input])    // 中文
/// tf("“{2}” not found by {1}", &[distro, input])   // 英文（语序可不同）
/// ```
///
/// 参数缺失时占位符原样保留，便于暴露漏译/漏传。
pub fn tf(msgid: &str, args: &[&str]) -> String {
    let template = t(msgid);
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template.as_str();

    while let Some(pos) = rest.find('{') {
        // 尝试解析 {n}
        let after = &rest[pos + 1..];
        let digits_end = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        let digits = &after[..digits_end];
        let closes = after[digits_end..].starts_with('}');

        if !digits.is_empty() && closes {
            out.push_str(&rest[..pos]);
            let idx: usize = digits.parse().unwrap_or(0);
            if idx >= 1 && idx <= args.len() {
                out.push_str(args[idx - 1]);
            } else {
                // 参数缺失：原样保留
                out.push_str(&rest[pos..pos + 1 + digits.len() + 1]);
            }
            rest = &after[digits.len() + 1..];
        } else {
            // 字面量 `{`：原样输出并继续
            out.push_str(&rest[..pos + 1]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_returns_msgid_without_translation() {
        // 测试环境未加载 .mo，gettext 原样返回 msgid
        assert_eq!(t("Run"), "Run");
    }

    #[test]
    fn tf_fills_numbered_placeholders() {
        assert_eq!(
            tf("{1} 找不到文件“{2}”。", &["Ubuntu", "a.txt"]),
            "Ubuntu 找不到文件“a.txt”。"
        );
    }

    #[test]
    fn tf_allows_reordered_placeholders() {
        // 译文可调换语序，这是位置记号相对 {} 的主要优势
        assert_eq!(
            tf("“{2}” not found ({1})", &["Ubuntu", "a.txt"]),
            "“a.txt” not found (Ubuntu)"
        );
    }

    #[test]
    fn tf_repeats_same_argument() {
        assert_eq!(tf("{1}-{1}", &["x"]), "x-x");
    }

    #[test]
    fn tf_keeps_missing_placeholder() {
        assert_eq!(tf("{1} 与 {2}", &["A"]), "A 与 {2}");
    }

    #[test]
    fn tf_handles_literal_braces() {
        assert_eq!(tf("set {x} = 1", &[]), "set {x} = 1");
    }

    /// 用进程内自增序号保证目录唯一：测试并行执行，固定路径会互相干扰。
    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "rd-i18n-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// 合并为一个测试：这几项都要读写进程级环境变量 `LANG`，
    /// 拆成多个 `#[test]` 并行跑会互相污染。
    #[test]
    fn has_catalog_checks_actual_catalog_presence() {
        let saved: Vec<(&str, Option<String>)> = ["LANG", "LC_ALL", "LC_MESSAGES", "LANGUAGE"]
            .iter()
            .map(|k| (*k, std::env::var(k).ok()))
            .collect();

        // --- 目录存在但只有别的程序的译文：不应命中 ---
        let tmp = tmp_dir("empty");
        let other = tmp.join("zh_CN/LC_MESSAGES");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("other-app.mo"), b"").unwrap();
        std::env::set_var("LANG", "zh_CN.UTF-8");
        assert!(!has_catalog(&tmp), "仅有其它程序的 .mo 时不应命中");

        // --- 放入本域的 .mo 后应命中 ---
        std::fs::write(other.join(format!("{TEXTDOMAIN}.mo")), b"").unwrap();
        assert!(has_catalog(&tmp), "存在本域 .mo 时应命中");
        let _ = std::fs::remove_dir_all(&tmp);

        // --- 基名回退：只提供 zh 目录，LANG=zh_CN.UTF-8 也应命中 ---
        let tmp2 = tmp_dir("base");
        let dir = tmp2.join("zh/LC_MESSAGES");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{TEXTDOMAIN}.mo")), b"").unwrap();
        std::env::set_var("LANG", "zh_CN.UTF-8");
        assert!(has_catalog(&tmp2), "应回退到语言基名 zh");
        let _ = std::fs::remove_dir_all(&tmp2);

        // --- 无任何 locale 环境变量时不应 panic ---
        let tmp3 = tmp_dir("none");
        std::fs::create_dir_all(&tmp3).unwrap();
        for k in ["LANG", "LC_ALL", "LC_MESSAGES", "LANGUAGE"] {
            std::env::remove_var(k);
        }
        let _ = has_catalog(&tmp3);
        let _ = std::fs::remove_dir_all(&tmp3);

        // 恢复环境
        for (k, v) in saved {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }
}
