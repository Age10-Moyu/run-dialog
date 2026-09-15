use adw::prelude::*;
use gtk::prelude::*;
use gtk::{
    gio, Application, Box as GtkBox, Button, Entry, FileDialog, Label, Orientation, Align,
};
use libadwaita as adw;
use std::cell::Cell;
use std::path::Path;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::OnceLock;

mod desktop;
mod elevate;
mod i18n;
mod launcher;
mod settings;

use i18n::{t, tf};

pub const APP_ID: &str = "com.Age10_Moyu.RunDialog";

/// 是否编译进调试用的隐藏参数（`--resolve` / `--elevate-info`）。
///
/// 这些是开发期排障入口，对最终用户没有用途，而且 `--resolve` 会打印
/// 用户输入、`--elevate-info` 会列出管理员账户，属于不必要的信息暴露面。
/// 因此只在 debug 构建启用；release 构建传入这些参数会被忽略。
///
/// `--askpass` **不受此限制**：它是提权流程的功能组成部分（`sudo -A`
/// 会调用它取密码），必须在 release 构建里可用。
pub const DEBUG_CLI: bool = cfg!(debug_assertions);

// ============================================================
//  config 模块
// ============================================================
pub mod config {
    use gtk::glib::{KeyFile, KeyFileFlags};
    use std::path::PathBuf;

    #[derive(Default, Clone)]
    pub struct Config {
        pub enable_win_compat: bool,
        /// 多个 `.desktop` 同样匹配时，是否弹出列表让用户选。
        /// 默认关闭：取第一个，行为可预测（同 Windows 运行框）。
        pub pick_desktop: bool,
    }

    fn config_path() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        home.join(".config/run-dialog/config.ini")
    }

    impl Config {
        pub fn load() -> Self {
            let path = config_path();
            let kf = KeyFile::new();
            if kf.load_from_file(&path, KeyFileFlags::NONE).is_ok() {
                Self {
                    enable_win_compat: kf
                        .boolean("experimental", "win_compat")
                        .unwrap_or(false),
                    pick_desktop: kf
                        .boolean("behavior", "pick_desktop")
                        .unwrap_or(false),
                }
            } else {
                Self::default()
            }
        }

        pub fn save(&self) -> Result<(), String> {
            let path = config_path();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let kf = KeyFile::new();
            kf.set_boolean("experimental", "win_compat", self.enable_win_compat);
            kf.set_boolean("behavior", "pick_desktop", self.pick_desktop);
            kf.save_to_file(&path).map_err(|e| e.to_string())?;
            Ok(())
        }
    }

    // ========================================================
    //  命令历史
    // ========================================================

    /// 历史记录最多保留条目数。
    ///
    /// 取值考虑：够用（能翻到常用的几十条），又不至于让文件无限增长。
    pub const HISTORY_LIMIT: usize = 100;

    fn history_path() -> PathBuf {
        config_path()
            .parent()
            .map(|p| p.join("history"))
            .unwrap_or_else(|| PathBuf::from("/tmp/run-dialog-history"))
    }

    /// 读取命令历史，**最近使用的在最后**。
    ///
    /// 文件格式：每行一条，`\n` 分隔。首次使用时文件不存在，返回空表。
    /// 读取失败（权限等）同样返回空表——历史不是关键功能，不该影响启动。
    pub fn load_history() -> Vec<String> {
        let Ok(content) = std::fs::read_to_string(history_path()) else {
            return Vec::new();
        };
        content
            .lines()
            .map(|l| l.trim_end_matches('\r').to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    /// 把一条命令追加进历史。
    ///
    /// 行为：
    /// - 连续重复的输入不重复记录（与 shell 一致）：若与最后一条相同则忽略
    /// - 内容里的换行会被替换为空格，避免破坏「一行一条」的格式
    /// - 超出 `HISTORY_LIMIT` 时丢弃最旧的
    ///
    /// 写入失败静默忽略：历史不是关键功能。
    pub fn push_history(command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        // 去掉换行，保持单行格式
        let sanitized: String = command
            .chars()
            .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
            .collect();

        let mut history = load_history();
        if history.last().map(|s| s.as_str()) == Some(sanitized.as_str()) {
            return;
        }
        history.push(sanitized);
        if history.len() > HISTORY_LIMIT {
            let drop_n = history.len() - HISTORY_LIMIT;
            history.drain(0..drop_n);
        }

        let path = history_path();
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        let mut out = history.join("\n");
        out.push('\n');
        let _ = std::fs::write(path, out);
    }

    /// 清空历史（设置页的「恢复默认」会用到）。
    pub fn clear_history() -> Result<(), String> {
        let path = history_path();
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

// ============================================================
//  win_compat 模块
// ============================================================
mod win_compat {
    use std::path::{Path, PathBuf};

    pub enum WinAction {
        Exec(PathBuf, Vec<String>),
        #[allow(dead_code)]
        OpenApp(String, Vec<String>),
    }

    struct Platform {
        home_root: &'static str,
        app_root: &'static str,
        appdata_roaming: &'static str,
        appdata_local: &'static str,
        appdata_locallow: &'static str,
        etc_root: &'static str,
        isolated_root: &'static str,
    }

    #[cfg(target_os = "linux")]
    const PLATFORM: Platform = Platform {
        home_root: "/home",
        app_root: "/usr/lib",
        appdata_roaming: ".config",
        appdata_local: ".local/share",
        appdata_locallow: ".local/state",
        etc_root: "/etc",
        isolated_root: ".local/share/win-compat/Windows",
    };

    #[cfg(target_os = "macos")]
    const PLATFORM: Platform = Platform {
        home_root: "/Users",
        app_root: "/Applications",
        appdata_roaming: "Library/Application Support",
        appdata_local: "Library/Caches",
        appdata_locallow: "Library/Caches",
        etc_root: "/etc",
        isolated_root: "Library/Application Support/win-compat/Windows",
    };

    pub fn try_win_path(input: &str) -> Option<WinAction> {
        let (first, args) = split_first_token(input.trim())?;
        if !is_win_path(&first) {
            return None;
        }

        let translated = translate_path(&first)?;

        if let Some(action) = translate_executable(&translated, &args) {
            return Some(action);
        }

        if translated.exists() {
            let mut a = vec![translated.to_string_lossy().into_owned()];
            a.extend(args);
            return Some(WinAction::Exec(PathBuf::from("open_default"), a));
        }

        let basename = translated.file_name()?.to_string_lossy().into_owned();
        let basename = basename.trim_end_matches(".exe").to_string();
        if let Some(p) = which(&basename) {
            return Some(WinAction::Exec(p, args));
        }

        None
    }

    fn split_first_token(s: &str) -> Option<(String, Vec<String>)> {
        let s = s.trim_start();
        if s.is_empty() {
            return None;
        }
        let (first, rest) = if let Some(stripped) = s.strip_prefix('"') {
            let end = stripped.find('"')?;
            (stripped[..end].to_string(), &stripped[end + 1..])
        } else {
            match s.find(char::is_whitespace) {
                Some(i) => (s[..i].to_string(), &s[i..]),
                None => (s.to_string(), ""),
            }
        };
        let args = rest.split_whitespace().map(|x| x.to_string()).collect();
        Some((first, args))
    }

    fn is_win_path(s: &str) -> bool {
        let lower = s.to_lowercase();
        lower.starts_with("c:\\") || lower.starts_with("c:/")
    }

    fn translate_path(input: &str) -> Option<PathBuf> {
        let rest = input
            .strip_prefix("C:\\")
            .or_else(|| input.strip_prefix("c:\\"))
            .or_else(|| input.strip_prefix("C:/"))
            .or_else(|| input.strip_prefix("c:/"))?;

        let normalized = rest.replace('\\', "/");
        let parts: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();

        if parts.is_empty() {
            return Some(PathBuf::from("/"));
        }

        let home = std::env::var_os("HOME").map(PathBuf::from);

        if parts[0].eq_ignore_ascii_case("users")
            && parts.len() >= 4
            && parts[2].eq_ignore_ascii_case("appdata")
        {
            let sub = parts[3];
            let tail = &parts[4..];
            let home_dir = home.as_ref()?;
            let base = if sub.eq_ignore_ascii_case("roaming") {
                home_dir.join(PLATFORM.appdata_roaming)
            } else if sub.eq_ignore_ascii_case("local") {
                home_dir.join(PLATFORM.appdata_local)
            } else if sub.eq_ignore_ascii_case("locallow") {
                home_dir.join(PLATFORM.appdata_locallow)
            } else {
                home_dir.join(PLATFORM.appdata_local).join(sub)
            };
            return Some(join_all(base, tail));
        }

        if parts[0].eq_ignore_ascii_case("users") && parts.len() >= 2 {
            let user = parts[1];
            let home_dir = home.as_ref()?;
            let current = home_dir.file_name()?.to_string_lossy().into_owned();
            let user_dir = if user.eq_ignore_ascii_case(&current) {
                home_dir.clone()
            } else {
                PathBuf::from(PLATFORM.home_root).join(user)
            };
            return Some(join_all(user_dir, &parts[2..]));
        }

        if parts.len() >= 4
            && parts[0].eq_ignore_ascii_case("windows")
            && parts[1].eq_ignore_ascii_case("system32")
            && parts[2].eq_ignore_ascii_case("drivers")
            && parts[3].eq_ignore_ascii_case("etc")
        {
            return Some(join_all(PathBuf::from(PLATFORM.etc_root), &parts[4..]));
        }

        if parts.len() >= 2
            && parts[0].eq_ignore_ascii_case("windows")
            && (parts[1].eq_ignore_ascii_case("system32")
                || parts[1].eq_ignore_ascii_case("syswow64"))
        {
            return Some(join_all(PathBuf::from("/usr/bin"), &parts[2..]));
        }

        if parts[0].eq_ignore_ascii_case("windows") {
            let base = home.as_ref()?.join(PLATFORM.isolated_root);
            return Some(join_all(base, &parts[1..]));
        }

        if parts[0].eq_ignore_ascii_case("program files (x86)")
            || parts[0].eq_ignore_ascii_case("program files")
        {
            let base = if cfg!(target_os = "macos") {
                PathBuf::from(PLATFORM.app_root)
            } else {
                home.as_ref()?.join("snap")
            };
            return Some(join_all(base, &parts[1..]));
        }

        Some(join_all(PathBuf::from("/"), &parts))
    }

    fn join_all(base: PathBuf, tail: &[&str]) -> PathBuf {
        let mut p = base;
        for seg in tail {
            p.push(seg);
        }
        p
    }

    #[cfg(target_os = "linux")]
    fn translate_executable(translated: &Path, args: &[String]) -> Option<WinAction> {
        let name = translated.file_name()?.to_string_lossy().to_lowercase();
        let name = name.trim_end_matches(".exe").to_string();

        let target: Option<&str> = match name.as_str() {
            "cmd" | "command" => Some("bash"),
            "powershell" | "pwsh" => Some("pwsh"),
            "notepad" => Some("gnome-text-editor"),
            "explorer" => Some("nautilus"),
            "calc" => Some("gnome-calculator"),
            "taskmgr" => Some("gnome-system-monitor"),
            "control" => Some("gnome-control-center"),
            "regedit" => Some("dconf-editor"),
            _ => None,
        };

        if let Some(cmd) = target {
            if name == "cmd"
                && !args.is_empty()
                && (args[0].eq_ignore_ascii_case("/c")
                    || args[0].eq_ignore_ascii_case("/k"))
            {
                let joined = args[1..].join(" ");
                return Some(WinAction::Exec(
                    PathBuf::from("bash"),
                    vec!["-c".into(), joined],
                ));
            }
            if name == "control" && args.is_empty() {
                return Some(WinAction::Exec(
                    PathBuf::from("gnome-control-center"),
                    vec![],
                ));
            }
            let path = which(cmd).unwrap_or_else(|| PathBuf::from(cmd));
            return Some(WinAction::Exec(path, args.to_vec()));
        }

        None
    }

    #[cfg(target_os = "macos")]
    fn translate_executable(translated: &Path, args: &[String]) -> Option<WinAction> {
        let name = translated.file_name()?.to_string_lossy().to_lowercase();
        let name = name.trim_end_matches(".exe").to_string();

        let command_map: Option<&str> = match name.as_str() {
            "cmd" | "command" => Some("/bin/zsh"),
            "powershell" | "pwsh" => Some("pwsh"),
            _ => None,
        };
        if let Some(cmd) = command_map {
            return Some(WinAction::Exec(PathBuf::from(cmd), args.to_vec()));
        }

        let app_map: Option<&str> = match name.as_str() {
            "notepad" => Some("TextEdit"),
            "explorer" => Some("Finder"),
            "calc" => Some("Calculator"),
            "control" => Some("System Settings"),
            "regedit" => Some("System Settings"),
            "taskmgr" => Some("Activity Monitor"),
            _ => None,
        };
        if let Some(app) = app_map {
            return Some(WinAction::OpenApp(app.to_string(), args.to_vec()));
        }

        None
    }

    fn which(prog: &str) -> Option<PathBuf> {
        let output = std::process::Command::new("which")
            .arg(prog)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(PathBuf::from(s))
        }
    }
}

// ============================================================
//  main
// ============================================================

fn main() -> gtk::glib::ExitCode {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("rundll32.exe")
            .args(["shell32.dll,#61"])
            .spawn();
        return gtk::glib::ExitCode::SUCCESS;
    }

    #[cfg(not(target_os = "windows"))]
    {
        // 先初始化 i18n，之后所有 t()/tf() 才会按当前 locale 生效
        i18n::init();

        let args: Vec<String> = std::env::args().collect();

        if args.len() == 2 && looks_like_windows_uri(&args[1]) {
            dispatch_windows_uri(&args[1]);
            return gtk::glib::ExitCode::SUCCESS;
        }

        let settings_mode = args.iter().any(|a| a == "--settings");

        // askpass 模式：由 `sudo -A` 调用，从临时文件读出密码写到 stdout。
        // 必须在创建 GTK Application 之前返回——sudo 只关心 stdout。
        if args.iter().any(|a| a == "--askpass") {
            return gtk::glib::ExitCode::from(elevate::run_askpass_mode());
        }

        // 调试模式：打印提权环境信息，便于排查预填与权限判断。
        // 仅 debug 构建可用（见 `DEBUG_CLI`）。
        if DEBUG_CLI && args.iter().any(|a| a == "--elevate-info") {
            println!("current_user      = {:?}", elevate::current_user());
            println!("is_root           = {}", elevate::is_root());
            println!("is_admin          = {}", elevate::current_user_is_admin());
            println!("default_user      = {:?}", elevate::default_admin_user());
            println!("sudo_available    = {:?}", elevate::sudo_available());
            println!("admin_accounts    = {:?}", elevate::admin_accounts());
            return gtk::glib::ExitCode::SUCCESS;
        }

        // 调试模式：`run-dialog --resolve <输入>...` 只打印解析结果，不启动界面。
        // 用于端到端验证查找逻辑。仅 debug 构建可用（见 `DEBUG_CLI`）。
        // 加 `--pick` 可强制开启「多候选时选择」策略以便观察。
        if DEBUG_CLI {
            if let Some(pos) = args.iter().position(|a| a == "--resolve") {
                let force_pick = args.iter().any(|a| a == "--pick");
                let resolver = if force_pick {
                    launcher::Resolver::with_pick(true)
                } else {
                    launcher::Resolver::new()
                };
                for input in &args[pos + 1..] {
                    if input.starts_with("--") {
                        continue;
                    }
                    println!("{:?}  =>  {:?}", input, resolver.resolve(input));
                }
                return gtk::glib::ExitCode::SUCCESS;
            }
        }

        let app = Application::builder()
            .application_id(APP_ID)
            .build();

        app.connect_startup(|_| {
            gtk::glib::set_application_name(&t("Run"));
        });

        app.connect_activate(move |app| {
            if settings_mode {
                settings::build_settings_window(app);
            } else {
                build_ui(app);
            }
        });

        app.run_with_args(&["run-dialog"]);
        gtk::glib::ExitCode::SUCCESS
    }
}

// ============================================================
//  主窗口
// ============================================================

fn build_ui(app: &Application) {
    let win_compat_enabled = config::Config::load().enable_win_compat;

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(&t("Run"))
        .default_width(520)
        .resizable(false)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let main_box = GtkBox::new(Orientation::Vertical, 16);
    main_box.set_margin_top(20);
    main_box.set_margin_bottom(20);
    main_box.set_margin_start(20);
    main_box.set_margin_end(20);

    let desc = Label::new(Some(&tf(
        "Type the name of a program, folder, document, or Internet resource, and {1} will open it for you.",
        &[distro_name()],
    )));
    desc.set_wrap(true);
    desc.set_xalign(0.0);
    main_box.append(&desc);

    let entry_box = GtkBox::new(Orientation::Horizontal, 8);
    entry_box.add_css_class("card");

    let entry = Entry::new();
    entry.set_hexpand(true);
    entry.set_margin_end(12);
    entry.set_margin_top(10);
    entry.set_margin_bottom(10);

    let entry_label = Label::new(Some(&t("Open (_O):")));
    entry_label.set_use_underline(true);
    entry_label.set_mnemonic_widget(Some(&entry));
    entry_label.set_margin_start(12);
    entry_label.set_margin_top(10);
    entry_label.set_margin_bottom(10);

    entry_box.append(&entry_label);
    entry_box.append(&entry);
    main_box.append(&entry_box);

    // 提权提示行：
    // - 普通用户 → 复选框「以管理员身份运行程序」（勾选后走自写 UAC）
    // - 已是 root → 盾牌图标 + 说明文字（纯提示，无可选项）
    let elevate_requested = Rc::new(Cell::new(false));
    if elevate::is_root() {
        let row = GtkBox::new(Orientation::Horizontal, 8);
        row.set_margin_start(4);

        let shield = gtk::Image::from_icon_name("security-high-symbolic");
        shield.set_tooltip_text(Some(&t("Creating this task with administrative privileges.")));
        row.append(&shield);

        let label = Label::new(Some(&t("Creating this task with administrative privileges.")));
        label.set_xalign(0.0);
        label.set_wrap(true);
        row.append(&label);

        main_box.append(&row);
    } else if elevate::sudo_available().is_some() {
        let check = gtk::CheckButton::with_label(&t("Run this program as an administrator"));
        check.set_margin_start(4);
        let flag = elevate_requested.clone();
        check.connect_toggled(move |c| flag.set(c.is_active()));
        main_box.append(&check);
    }

    // ---- 内嵌候选列表（默认隐藏）----
    //
    // 早先多候选时弹独立的模态窗口，用户得先看清列表再点选，多一次视线转移。
    // 现在直接在输入框下方展开列表：与输入内容紧邻，视觉关联更明确，
    // 也不会遮挡主窗口。
    let pick_list = gtk::ListBox::new();
    pick_list.set_selection_mode(gtk::SelectionMode::None);
    pick_list.add_css_class("boxed-list");
    pick_list.set_visible(false);

    let pick_scroll = gtk::ScrolledWindow::new();
    pick_scroll.set_child(Some(&pick_list));
    pick_scroll.set_visible(false);
    // 候选多时限制高度，避免撑高窗口
    pick_scroll.set_max_content_height(220);
    pick_scroll.set_propagate_natural_height(true);
    pick_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

    main_box.append(&pick_scroll);

    let btn_box = GtkBox::new(Orientation::Horizontal, 8);
    btn_box.set_halign(Align::End);
    btn_box.set_margin_top(8);

    let ok_btn = Button::with_label(&t("OK"));
    ok_btn.add_css_class("suggested-action");
    ok_btn.set_sensitive(false);
    btn_box.append(&ok_btn);

    let cancel_btn = Button::with_label(&t("Cancel"));
    btn_box.append(&cancel_btn);

    let browse_btn = Button::with_mnemonic(&t("_Browse..."));
    btn_box.append(&browse_btn);

    main_box.append(&btn_box);
    toolbar_view.set_content(Some(&main_box));
    window.set_content(Some(&toolbar_view));

    // 输入变化时控制“确定”按钮可用性
    {
        let ok_btn = ok_btn.clone();
        entry.connect_changed(move |entry| {
            ok_btn.set_sensitive(!entry.text().trim().is_empty());
        });
    }

    // 命令历史：↑ / ↓ 翻阅之前的输入
    {
        // 历史快照 + 当前浏览位置 + 用户原始草稿。
        //
        // 位置语义：`cursor == history.len()` 表示「不在历史里」（正在编辑新输入），
        // 此时按 ↑ 从最后一条开始往回翻。
        //
        // 为什么需要 `navigating` 标志：`set_text()` 会触发 `connect_changed`，
        // 如果在 changed 里无条件重置 cursor，↑ 刚翻上去就被重置回来，
        // 历史就翻不动了。所以翻历史期间置位，changed 回调据此跳过重置。
        let history = Rc::new(config::load_history());
        let cursor = Rc::new(Cell::new(history.len()));
        let draft = Rc::new(std::cell::RefCell::new(String::new()));
        let navigating = Rc::new(Cell::new(false));

        let key_ctrl = gtk::EventControllerKey::new();
        {
            let entry = entry.clone();
            let history = history.clone();
            let cursor = cursor.clone();
            let draft = draft.clone();
            let navigating = navigating.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                let h = &*history;
                if h.is_empty() {
                    return gtk::glib::Propagation::Proceed;
                }
                match key {
                    gtk::gdk::Key::Up => {
                        // 首次按 ↑：记住用户原本的输入，回头能恢复
                        if cursor.get() >= h.len() {
                            *draft.borrow_mut() = entry.text().to_string();
                        }
                        if cursor.get() > 0 {
                            cursor.set(cursor.get() - 1);
                        }
                        navigating.set(true);
                        entry.set_text(&h[cursor.get()]);
                        navigating.set(false);
                        entry.set_position(-1); // 光标移到末尾
                        gtk::glib::Propagation::Stop
                    }
                    gtk::gdk::Key::Down => {
                        if cursor.get() >= h.len() {
                            return gtk::glib::Propagation::Proceed;
                        }
                        cursor.set(cursor.get() + 1);
                        navigating.set(true);
                        if cursor.get() >= h.len() {
                            // 翻回底部：恢复用户原本的输入
                            entry.set_text(&draft.borrow());
                        } else {
                            entry.set_text(&h[cursor.get()]);
                        }
                        navigating.set(false);
                        entry.set_position(-1);
                        gtk::glib::Propagation::Stop
                    }
                    _ => gtk::glib::Propagation::Proceed,
                }
            });
        }
        entry.add_controller(key_ctrl);

        // 用户手动编辑内容时退出历史浏览，下次按 ↑ 重新从末尾开始
        {
            let cursor = cursor.clone();
            let history = history.clone();
            let navigating = navigating.clone();
            entry.connect_changed(move |_| {
                if !navigating.get() {
                    cursor.set(history.len());
                }
            });
        }
    }

    {
        let window = window.clone();
        cancel_btn.connect_clicked(move |_| window.close());
    }

    // submit 闭包
    //
    // `pending` 保存当前展示的候选（query + 名字列表），供点击列表行时使用。
    // 为空表示当前没有候选在展示。
    let pending: Rc<std::cell::RefCell<Option<(String, Vec<String>)>>> =
        Rc::new(std::cell::RefCell::new(None));

    let submit = {
        let app = app.clone();
        let entry = entry.clone();
        let window = window.clone();
        let elevate_requested = elevate_requested.clone();
        let pick_list = pick_list.clone();
        let pick_scroll = pick_scroll.clone();
        let pending = pending.clone();
        move || {
            let cmd = entry.text().to_string();
            if cmd.trim().is_empty() {
                return;
            }
            let app = app.clone();
            let window = window.clone();

            // 记入历史（无论后续成功与否——用户的输入本身就值得记住；
            // 连续重复的输入会被 push_history 内部忽略）
            config::push_history(&cmd);

            // 勾选了「以管理员身份运行」：先弹 UAC 取密码，再提权启动
            if elevate_requested.get() && !elevate::is_root() {
                match resolve_for_elevation(&cmd, win_compat_enabled) {
                    Some((program, args)) => {
                        window.set_visible(false);
                        show_uac_dialog(&app, &window, &program, &args);
                    }
                    None => {
                        show_error_dialog(&app, &window, &not_found_message(&cmd));
                    }
                }
                return;
            }

            match execute_command(&cmd, win_compat_enabled) {
                Ok(Outcome::Done) => {
                    // 命令已交给系统执行，本窗口使命完成
                    window.destroy();
                }
                Ok(Outcome::NeedPick { query, candidates }) => {
                    // 内嵌展示候选，不弹独立窗口
                    show_inline_pick(&pick_list, &pick_scroll, &pending, &query, &candidates);
                }
                Err(msg) => {
                    // 失败时收起候选列表，并把错误框盖在主窗口上。
                    // 主窗口不隐藏：关闭错误框后输入内容仍在，可直接改。
                    clear_inline_pick(&pick_list, &pick_scroll, &pending);
                    show_error_dialog(&app, &window, &msg);
                }
            }
        }
    };

    // 点击候选行：执行选中的那项并关窗
    {
        let window = window.clone();
        let pending = pending.clone();
        let list_for_clear = pick_list.clone();
        let scroll_for_clear = pick_scroll.clone();
        let pending_for_clear = pending.clone();
        pick_list.connect_row_activated(move |_, row| {
            let idx = row.index();
            if idx < 0 {
                return;
            }
            let chosen = {
                let p = pending.borrow();
                let Some((_, names)) = p.as_ref() else {
                    return;
                };
                match names.get(idx as usize) {
                    Some(n) => n.clone(),
                    None => return,
                }
            };
            let query = {
                let p = pending.borrow();
                match p.as_ref() {
                    Some((q, _)) => q.clone(),
                    None => return,
                }
            };

            match execute_pick(&query, &chosen) {
                Ok(()) => window.destroy(),
                Err(msg) => {
                    clear_inline_pick(&list_for_clear, &scroll_for_clear, &pending_for_clear);
                    if let Some(app) = window.application() {
                        if let Ok(app) = app.downcast::<Application>() {
                            show_error_dialog(&app, &window, &msg);
                        }
                    }
                }
            }
        });
    }

    {
        let submit = submit.clone();
        ok_btn.connect_clicked(move |_| submit());
    }

    {
        // Enter 走 emit_clicked，禁用时自然不触发
        let ok_btn = ok_btn.clone();
        entry.connect_activate(move |_| {
            ok_btn.emit_clicked();
        });
    }

    {
        let entry = entry.clone();
        let window = window.clone();
        browse_btn.connect_clicked(move |_| {
            let dialog = FileDialog::new();
            dialog.set_title(&t("File"));
            let entry = entry.clone();
            dialog.open(
                Some(&window),
                gio::Cancellable::NONE,
                move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            entry.set_text(&path.to_string_lossy());
                        }
                    }
                },
            );
        });
    }

    window.present();
    entry.grab_focus();
}

// ============================================================
//  错误对话框（Windows 风格）
// ============================================================

/// 错误对话框（Windows 风格）。
///
/// `parent` 是发起本次执行的主窗口。错误框以模态子窗口的形式附着在它上面：
/// 关闭后主窗口仍在，用户可以修正输入后直接重试。
fn show_error_dialog(app: &Application, parent: &adw::ApplicationWindow, message: &str) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .transient_for(parent)
        .modal(true)
        .title(&t("Run"))
        .default_width(520)
        .default_height(180)
        .resizable(false)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content = GtkBox::new(Orientation::Horizontal, 24);
    content.set_margin_top(32);
    content.set_margin_bottom(24);
    content.set_margin_start(32);
    content.set_margin_end(32);
    content.set_valign(Align::Center);
    content.set_vexpand(true);

    let icon = gtk::Image::from_icon_name("dialog-error");
    icon.set_pixel_size(48);
    icon.set_valign(Align::Start);

    let label = Label::new(Some(message));
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.set_hexpand(true);

    content.append(&icon);
    content.append(&label);

    let btn_box = GtkBox::new(Orientation::Horizontal, 0);
    btn_box.set_halign(Align::End);
    btn_box.set_margin_top(8);
    btn_box.set_margin_bottom(24);
    btn_box.set_margin_end(28);

    let ok_btn = Button::with_label(&t("OK"));
    ok_btn.add_css_class("suggested-action");
    btn_box.append(&ok_btn);

    let outer = GtkBox::new(Orientation::Vertical, 0);
    outer.append(&content);
    outer.append(&btn_box);
    toolbar_view.set_content(Some(&outer));
    window.set_content(Some(&toolbar_view));

    {
        let window = window.clone();
        let parent = parent.clone();
        ok_btn.connect_clicked(move |_| {
            window.close();
            // 主窗口一直存在（只是被模态框挡住），这里确保它拿回焦点
            parent.present();
        });
    }

    // 点标题栏 ✕ 或按 Esc 关闭时同样让主窗口拿回焦点。
    // 注意：不能调用 app.quit()，否则整个应用退出，主窗口再也回不来。
    {
        let parent = parent.clone();
        window.connect_close_request(move |_| {
            parent.present();
            gtk::glib::Propagation::Proceed
        });
    }

    window.present();
}

// ============================================================
//  用户账户控制（UAC）
// ============================================================

/// 自写的 UAC 对话框：确认提权并索取管理员密码。
///
/// 布局对应 Windows 的「你要允许此应用对你的设备进行更改吗？」：
///
/// ```text
/// ┌ 用户账户控制 ───────────────────────────┐
/// │ 你要允许此应用对你的设备进行更改吗？      │  ← 大字
/// │                                          │
/// │ [盾牌]  [AppName]                        │
/// │         已验证的发布者: [Author]         │
/// │                                          │
/// │ 显示更多详细信息                          │  ← Expander
/// │                                          │
/// │ 若要继续，请输入管理员用户名和密码。      │
/// │ [用户名                              ]   │
/// │ [密码                                ]   │
/// ├──────────────────────────────────────────┤
/// │                    [   是   ] [   否   ] │
/// └──────────────────────────────────────────┘
/// ```
///
/// 展开后额外显示程序位置与发布者证书信息。
fn show_uac_dialog(
    app: &Application,
    parent: &adw::ApplicationWindow,
    program: &str,
    args: &[String],
) {
    // 应用名（.desktop 的 Name，取不到则用文件名）+ 发布者（包管理器反查）
    let app_name = describe_program(program);
    let publisher = elevate::publisher_of(program);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .transient_for(parent)
        .modal(true)
        .title(&t("User Account Control"))
        .default_width(560)
        .resizable(false)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content = GtkBox::new(Orientation::Vertical, 12);
    content.set_margin_top(20);
    content.set_margin_bottom(16);
    content.set_margin_start(24);
    content.set_margin_end(24);

    // ---- 标题（较大字号）----
    let heading = Label::new(Some(&t("Do you want to allow this app to make changes to your device?")));
    heading.set_wrap(true);
    heading.set_xalign(0.0);
    heading.add_css_class("title-3");
    content.append(&heading);

    // ---- 应用信息行：Logo + 名称 + 发布者 ----
    {
        let info = GtkBox::new(Orientation::Horizontal, 16);
        info.set_margin_top(8);

        let logo = gtk::Image::from_icon_name("application-x-executable");
        logo.set_pixel_size(48);
        logo.set_valign(Align::Start);
        info.append(&logo);

        let text = GtkBox::new(Orientation::Vertical, 4);
        let name_label = Label::new(Some(&app_name));
        name_label.set_xalign(0.0);
        name_label.add_css_class("heading");
        text.append(&name_label);

        // 发布者行。Linux 桌面程序没有 Authenticode 代码签名，
        // 所以「已验证」实际上不会出现——但仍按规范区分两种措辞，
        // 以便将来接入可信来源（如发行版签名验证）时无需改 UI。
        let publisher_label = if publisher.verified {
            let name = publisher
                .name
                .clone()
                .unwrap_or_else(|| t("Unknown publisher"));
            Label::new(Some(&tf("Verified publisher: {1}", &[&name])))
        } else {
            let name = publisher
                .name
                .clone()
                .unwrap_or_else(|| t("Unknown publisher"));
            Label::new(Some(&tf("Unverified publisher: {1}", &[&name])))
        };
        publisher_label.set_xalign(0.0);
        publisher_label.set_wrap(true);
        publisher_label.add_css_class("dim-label");
        text.append(&publisher_label);

        info.append(&text);
        content.append(&info);
    }

    // ---- 「显示详细信息」展开区 ----
    {
        // 标签需要随展开状态在「显示」/「隐藏」之间切换，所以传 Label 而非字符串
        let expander_label = Label::new(Some(&t("Show details (_D)")));
        expander_label.set_use_underline(true); // 让 (_D) 成为 Alt+D 助记符
        expander_label.set_xalign(0.0);

        let expander = gtk::Expander::new(None::<&str>);
        expander.set_label_widget(Some(&expander_label));
        expander.set_margin_top(4);

        // 展开时改成「隐藏详细信息」，折叠时改回「显示详细信息」。
        //
        // 注意：`set_text()` 之后必须重新打开 `use_underline`，否则 GTK 会把
        // 新文本当作纯文本渲染，`_D` 里的下划线就显示出来了（助记符失效）。
        {
            let expander_label = expander_label.clone();
            expander.connect_expanded_notify(move |e| {
                let text = if e.is_expanded() {
                    t("Hide details (_D)")
                } else {
                    t("Show details (_D)")
                };
                expander_label.set_text(&text);
                expander_label.set_use_underline(true);
            });
        }

        let details = GtkBox::new(Orientation::Vertical, 6);
        details.set_margin_start(12);
        details.set_margin_top(6);

        let loc_row = GtkBox::new(Orientation::Vertical, 2);
        let loc_title = Label::new(Some(&t("Program location:")));
        loc_title.set_xalign(0.0);
        loc_title.add_css_class("dim-label");
        loc_row.append(&loc_title);

        let loc_value = Label::new(Some(program));
        loc_value.set_xalign(0.0);
        loc_value.set_wrap(true);
        loc_value.set_selectable(true);
        loc_value.add_css_class("monospace");
        loc_row.append(&loc_value);
        details.append(&loc_row);

        // 原文位置在这里显示签名状态。Windows 上是一句可点击链接
        // （「显示有关此发布者的证书的信息」）；Linux 没有 Authenticode
        // 签名体系，也没有可查看的证书，所以改为一句**纯文本**陈述。
        let cert_note = Label::new(Some(&t("This program is not digitally signed.")));
        cert_note.set_xalign(0.0);
        cert_note.set_wrap(true);
        cert_note.add_css_class("dim-label");
        details.append(&cert_note);

        expander.set_child(Some(&details));
        content.append(&expander);
    }

    // ---- 提示 + 用户名 / 密码 ----
    let prompt = Label::new(Some(&t("To continue, type an administrator user name and password.")));
    prompt.set_wrap(true);
    prompt.set_xalign(0.0);
    prompt.set_margin_top(8);
    content.append(&prompt);

    // 预填规则：当前用户本身是管理员就填自己，否则填最早创建的管理员账户
    let default_user = elevate::default_admin_user();

    let user_entry = Entry::new();
    user_entry.set_text(&default_user);
    user_entry.set_placeholder_text(Some(&t("User name")));
    content.append(&user_entry);

    let pass_entry = Entry::new();
    pass_entry.set_visibility(false);
    pass_entry.set_input_purpose(gtk::InputPurpose::Password);
    pass_entry.set_placeholder_text(Some(&t("Password")));
    content.append(&pass_entry);

    // 认证失败提示。初始隐藏，失败时显示在密码框下方（与 Windows 一致：
    // 留在同一对话框里让用户改密码重试，而不是关掉整个界面）。
    let error_label = Label::new(None);
    error_label.set_xalign(0.0);
    error_label.set_wrap(true);
    error_label.set_visible(false);
    error_label.add_css_class("error");
    content.append(&error_label);

    // ---- 按钮：是 / 否 ----
    let btn_box = GtkBox::new(Orientation::Horizontal, 12);
    btn_box.set_halign(Align::End);
    btn_box.set_margin_top(12);
    btn_box.set_margin_end(24);
    btn_box.set_margin_bottom(20);

    let yes_btn = Button::with_label(&t("Yes"));
    yes_btn.set_size_request(110, -1);
    btn_box.append(&yes_btn);

    let no_btn = Button::with_label(&t("No"));
    no_btn.set_size_request(110, -1);
    btn_box.append(&no_btn);

    let outer = GtkBox::new(Orientation::Vertical, 0);
    outer.append(&content);
    outer.append(&btn_box);
    toolbar_view.set_content(Some(&outer));
    window.set_content(Some(&toolbar_view));

    // ---- 交互 ----
    {
        let window = window.clone();
        let parent = parent.clone();
        no_btn.connect_clicked(move |_| {
            window.close();
            parent.set_visible(true);
            parent.present();
        });
    }

    {
        let window = window.clone();
        let parent = parent.clone();
        let app = app.clone();
        let user_entry = user_entry.clone();
        let pass_entry = pass_entry.clone();
        let error_label = error_label.clone();
        let yes_btn_clone = yes_btn.clone();
        let program = program.to_string();
        let args = args.to_vec();
        yes_btn.connect_clicked(move |_| {
            let user = user_entry.text().to_string();
            let mut password = pass_entry.text().to_string();
            if user.trim().is_empty() || password.is_empty() {
                // 未填完不提交（此时也要擦掉刚取出的副本）
                elevate::wipe_string(&mut password);
                return;
            }

            // 立刻清空 Entry，缩短密码在控件缓冲区里的驻留时间
            pass_entry.set_text("");

            // 锁定按钮，避免认证期间重复点击
            yes_btn_clone.set_sensitive(false);
            error_label.set_visible(false);

            // 认证在后台线程做（会阻塞等 sudo 退出），结果通过通道回到 UI 线程。
            // 不能直接在 GTK 主线程里等待——那会把界面冻住。
            let (tx, rx) = std::sync::mpsc::channel::<elevate::AuthOutcome>();

            {
                let program = program.clone();
                let args = args.clone();
                let mut password = std::mem::take(&mut password);
                std::thread::spawn(move || {
                    let outcome = elevate_and_authenticate(&user, &password, &program, &args);
                    // 无论成败，认证阶段结束后立即擦除本线程持有的密码副本
                    elevate::wipe_string(&mut password);
                    let _ = tx.send(outcome);
                });
            }

            // 保持窗口响应：用短周期轮询把结果搬回 UI 线程
            {
                let window = window.clone();
                let parent = parent.clone();
                let pass_entry = pass_entry.clone();
                let error_label = error_label.clone();
                let yes_btn = yes_btn_clone.clone();
                gtk::glib::timeout_add_local(
                    std::time::Duration::from_millis(120),
                    move || match rx.try_recv() {
                        Ok(elevate::AuthOutcome::Ok) => {
                            window.close();
                            parent.destroy();
                            gtk::glib::ControlFlow::Break
                        }
                        Ok(elevate::AuthOutcome::Failed(msg)) => {
                            // 留在 UAC 里让用户改密码重试，与 Windows 行为一致
                            error_label.set_text(&tf(
                                "Authentication failed: {1}",
                                &[&msg],
                            ));
                            error_label.set_visible(true);
                            pass_entry.set_text("");
                            pass_entry.grab_focus();
                            yes_btn.set_sensitive(true);
                            gtk::glib::ControlFlow::Break
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            gtk::glib::ControlFlow::Continue
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            error_label.set_text(&t("Authentication failed."));
                            error_label.set_visible(true);
                            yes_btn.set_sensitive(true);
                            gtk::glib::ControlFlow::Break
                        }
                    },
                );
            }
            let _ = &app;
        });
    }

    // 回车直接提交
    {
        let yes_btn = yes_btn.clone();
        pass_entry.connect_activate(move |_| yes_btn.emit_clicked());
    }

    {
        let parent = parent.clone();
        window.connect_close_request(move |_| {
            parent.set_visible(true);
            parent.present();
            gtk::glib::Propagation::Proceed
        });
    }

    window.present();
    pass_entry.grab_focus();
}

// ============================================================
//  内嵌候选列表
// ============================================================

/// 在主窗口内展开候选列表（替代早先的独立模态窗口）。
///
/// 列表紧邻输入框，视觉关联明确；用户点某一行即执行该项。
/// 列表内容同时记入 `pending`，供行激活回调查出对应的显示名。
fn show_inline_pick(
    list: &gtk::ListBox,
    scroll: &gtk::ScrolledWindow,
    pending: &Rc<std::cell::RefCell<Option<(String, Vec<String>)>>>,
    query: &str,
    candidates: &[String],
) {
    // 清掉上一轮的行
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    for name in candidates {
        let row = adw::ActionRow::builder()
            .title(name)
            .activatable(true)
            .build();
        let chevron = gtk::Image::from_icon_name("go-next-symbolic");
        row.add_suffix(&chevron);
        list.append(&row);
    }

    *pending.borrow_mut() = Some((query.to_string(), candidates.to_vec()));

    list.set_visible(true);
    scroll.set_visible(true);
}

/// 收起候选列表并清除状态。
fn clear_inline_pick(
    list: &gtk::ListBox,
    scroll: &gtk::ScrolledWindow,
    pending: &Rc<std::cell::RefCell<Option<(String, Vec<String>)>>>,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    *pending.borrow_mut() = None;
    list.set_visible(false);
    scroll.set_visible(false);
}

// ============================================================
//  签名信息面板
// ============================================================

/// 没有可用证书时，展示我们能确知的事实。
///
/// 当前 UAC 里已改为一句「此程序未签名。」纯文本，不再提供证书入口，
/// 所以这个面板**暂无调用者**。保留原因：如果将来接入发行版签名验证
/// （如 `debsig-verify`）或用户想查看程序详情，这是现成的展示层。
///
/// 关键原则：**不宣称「已签名」或「未签名」**。Linux 桌面程序普遍没有
/// Authenticode 式的代码签名，我们无从判断，就不该给用户误导性的结论。
/// 这里列出的都是可验证的客观信息。
#[allow(dead_code)]
fn show_signature_dialog(
    app: Option<&Application>,
    parent: &adw::ApplicationWindow,
    program: &str,
) {
    let Some(app) = app else { return };
    let info = elevate::signature_info(program);
    let publisher = elevate::publisher_of(program);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .transient_for(parent)
        .modal(true)
        .title(&t("Publisher information"))
        .default_width(520)
        .resizable(false)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content = GtkBox::new(Orientation::Vertical, 16);
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.set_margin_start(24);
    content.set_margin_end(24);

    // 说明：明确告知本平台没有代码签名体系，避免被误读为「已验证」
    let note = Label::new(Some(&t(
        "This platform does not provide code signing for desktop applications. The information below is descriptive only and does not verify the publisher.",
    )));
    note.set_wrap(true);
    note.set_xalign(0.0);
    note.add_css_class("dim-label");
    content.append(&note);

    let group = adw::PreferencesGroup::new();
    group.set_title(&t("Program"));

    let add_row = |title: &str, value: Option<String>| {
        if let Some(v) = value.filter(|s| !s.is_empty()) {
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle(&v)
                .subtitle_selectable(true)
                .build();
            group.add(&row);
        }
    };

    add_row(&t("Path"), Some(program.to_string()));
    add_row(
        &t("Resolved path"),
        info.resolved_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
    );

    // 发布者及其来源：让用户知道这个信息是怎么来的，才好判断可信度
    let source_desc = match publisher.source {
        elevate::PublisherSource::ElfNote => t("Read from the binary's own metadata"),
        elevate::PublisherSource::PackageManager => t("Looked up via the package manager"),
        elevate::PublisherSource::Unknown => t("Could not be determined"),
    };
    add_row(
        &t("Publisher"),
        Some(match &publisher.name {
            Some(n) => format!("{n}\n({source_desc})"),
            None => source_desc,
        }),
    );

    add_row(&t("File type"), Some(info.file_kind.clone()));
    add_row(
        &t("Size"),
        info.file_size.map(|s| format!("{s} bytes")),
    );
    // build-id 是链接时生成的内容哈希，**不是签名**，故明确标注用途
    add_row(
        &t("Build ID"),
        info.build_id
            .as_ref()
            .map(|id| format!("{id}\n({})", t("Not a signature"))),
    );
    add_row(
        &t("Desktop entry"),
        info.desktop_file
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
    );
    content.append(&group);

    let btn_box = GtkBox::new(Orientation::Horizontal, 0);
    btn_box.set_halign(Align::End);

    let close_btn = Button::with_label(&t("OK"));
    close_btn.add_css_class("suggested-action");
    btn_box.append(&close_btn);

    let outer = GtkBox::new(Orientation::Vertical, 0);
    outer.append(&content);
    outer.append(&btn_box);
    toolbar_view.set_content(Some(&outer));
    window.set_content(Some(&toolbar_view));

    {
        let window = window.clone();
        close_btn.connect_clicked(move |_| window.close());
    }

    window.present();
}

/// 认证并提权启动（UI 侧薄封装）。
///
/// `_user` 目前仅用于显示（sudo 以当前用户身份验证并提权到 root），
/// 保留参数以对应 UAC 里的用户名输入框。
///
/// 认证结果的判断、密码错误等情况的识别，都在
/// `elevate::elevate_and_authenticate()` 里。早先这里是**无条件返回成功**的，
/// 导致密码错误时界面直接消失且没有任何提示。
///
/// 本函数会阻塞至多约 1.2 秒，**必须**在后台线程调用。
fn elevate_and_authenticate(
    _user: &str,
    password: &str,
    program: &str,
    args: &[String],
) -> elevate::AuthOutcome {
    let _ = _user;
    elevate::elevate_and_authenticate(password, program, args)
}

/// 根据程序路径推断**应用显示名**。
///
/// 在 `.desktop` 索引里找 `Exec=` 首程序匹配的条目，取其 `Name`；
/// 找不到就用文件名。
///
/// 注意：发布者**不**在这里获取——`.desktop` 里没有发布者字段，
/// 早先版本误用了 `Icon=`，那是图标名而不是发布者。发布者改由
/// `elevate::publisher_of()` 通过包管理器反查。
fn describe_program(program: &str) -> String {
    let basename = Path::new(program)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string());

    let idx = desktop::DesktopIndex::scan();
    for entry in idx.iter() {
        if let Some(exec) = entry.exec.as_deref() {
            if let Some(argv) = shlex::split(exec) {
                if let Some(first) = argv.first() {
                    let same = Path::new(first)
                        .file_name()
                        .map(|s| s.to_string_lossy() == basename)
                        .unwrap_or(false);
                    if same {
                        return entry.name.clone();
                    }
                }
            }
        }
    }

    basename
}

// ============================================================
//  命令执行
// ============================================================

/// `execute_command` 的结果。
///
/// 需要区分「已完成」与「需要用户在候选里选一个」，因为后者必须由 UI 层
/// 弹框处理，不能在这里自行决定。
enum Outcome {
    /// 已交给系统执行
    Done,
    /// 有多个候选，需要用户选择
    NeedPick {
        query: String,
        candidates: Vec<String>,
    },
}

/// 解析并执行用户输入。
///
/// 顺序：Windows URI → Windows 兼容层（实验）→ launcher 的 7 步查找。
fn execute_command(command: &str, win_compat_enabled: bool) -> Result<Outcome, String> {
    let command = command.trim();
    if command.is_empty() {
        return Ok(Outcome::Done);
    }

    // 0. Windows URI（ms-settings: / control:）
    if looks_like_windows_uri(command) {
        dispatch_windows_uri(command);
        return Ok(Outcome::Done);
    }

    // 1. Windows 兼容层（实验开关）
    if win_compat_enabled {
        if let Some(action) = win_compat::try_win_path(command) {
            match action {
                win_compat::WinAction::Exec(prog, args) => {
                    if prog == Path::new("open_default") {
                        // translate_path 里没找到 executable 时的占位
                        if let Some(first) = args.first() {
                            let _ = open_default(first)
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn();
                            return Ok(Outcome::Done);
                        }
                    } else {
                        let _ = Command::new(&prog)
                            .args(&args)
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .spawn();
                        return Ok(Outcome::Done);
                    }
                }
                win_compat::WinAction::OpenApp(app, args) => {
                    let _ = Command::new("open")
                        .args(["-a", &app])
                        .args(&args)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn();
                    return Ok(Outcome::Done);
                }
            }
        }
        if is_windows_drive_path(command) {
            return Err(not_found_message(command));
        }
    }

    // 2. 常规查找：URL / 路径 / cwd / 系统目录 / PATH / .desktop / 兜底
    let resolver = launcher::Resolver::new();
    let Some(action) = resolver.resolve(command) else {
        return Err(not_found_message(command));
    };

    // 多候选：交给 UI 弹选择框
    if let Some((query, candidates)) = action.pick_candidates() {
        return Ok(Outcome::NeedPick {
            query: query.to_string(),
            candidates: candidates.to_vec(),
        });
    }

    action.run().map_err(|_| not_found_message(command))?;
    Ok(Outcome::Done)
}

/// 执行用户在候选列表中选中的项。
fn execute_pick(query: &str, chosen: &str) -> Result<(), String> {
    let resolver = launcher::Resolver::new();
    match resolver.resolve_pick(query, chosen) {
        Some(action) => action.run(),
        None => Err(not_found_message(query)),
    }
}

/// 为提权启动解析出「程序 + 参数」。
///
/// 只接受 `Launch`：URL（`Open`）和 curl/wget 之类的 shell 场景没有
/// 「以管理员身份运行」的语义，多候选（`Pick`）也不适合在提权流程里
/// 再插一层选择，所以这些情况一律返回 `None`。
///
/// 已是 root 时不需要提权，调用方应先判 `elevate::is_root()`。
fn resolve_for_elevation(command: &str, win_compat_enabled: bool) -> Option<(String, Vec<String>)> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }

    // Windows 兼容层若命中，直接用它翻译出的程序
    if win_compat_enabled {
        if let Some(win_compat::WinAction::Exec(prog, args)) = win_compat::try_win_path(command) {
            if prog != Path::new("open_default") {
                return Some((prog.to_string_lossy().into_owned(), args));
            }
        }
    }

    let resolver = launcher::Resolver::with_pick(false);
    match resolver.resolve(command) {
        Some(launcher::Action::Launch { program, args, .. }) => Some((program, args)),
        _ => None,
    }
}
/// 是否为 `C:\...` 形式的 Windows 盘符路径。
fn is_windows_drive_path(s: &str) -> bool {
    let lower = s.to_lowercase();
    lower.starts_with("c:\\") || lower.starts_with("c:/")
}

fn not_found_message(input: &str) -> String {
    tf(
        "{1} cannot find '{2}'. Make sure you typed the name correctly, and then try it again.",
        &[distro_name(), input],
    )
}

// ============================================================
//  平台辅助函数
// ============================================================

#[cfg(target_os = "linux")]
fn distro_name() -> &'static str {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE.get_or_init(|| {
        std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|content| {
                content
                    .lines()
                    .find(|line| line.starts_with("NAME="))
                    .map(|line| {
                        line.trim_start_matches("NAME=")
                            .trim_matches('"')
                            .to_string()
                    })
            })
            .unwrap_or_else(|| "Linux".to_string())
    })
}

#[cfg(target_os = "macos")]
fn distro_name() -> &'static str {
    "macOS"
}

#[cfg(target_os = "windows")]
fn distro_name() -> &'static str {
    "Windows"
}
#[cfg(target_os = "linux")]
fn open_default(target: &str) -> Command {
    let mut c = Command::new("xdg-open");
    c.arg(target);
    c
}

#[cfg(target_os = "macos")]
fn open_default(target: &str) -> Command {
    let mut c = Command::new("open");
    c.arg(target);
    c
}


// ============================================================
//  Windows URI 分发
// ============================================================

fn looks_like_windows_uri(s: &str) -> bool {
    let lower = s.to_lowercase();
    lower.starts_with("ms-settings:") || lower.starts_with("control:")
}

#[cfg(target_os = "linux")]
fn dispatch_windows_uri(uri: &str) {
    let lower = uri.to_lowercase();

    if lower.starts_with("control:") {
        let _ = Command::new("gnome-control-center").spawn();
        return;
    }

    let key = lower.trim_start_matches("ms-settings:").trim_end_matches('/');

    let panel = if key.starts_with("display") {
        "display"
    } else if key.starts_with("sound") || key.starts_with("audio") {
        "sound"
    } else if key.starts_with("network") || key.starts_with("wifi") || key.starts_with("ethernet") {
        "network"
    } else if key.starts_with("bluetooth") {
        "bluetooth"
    } else if key.starts_with("keyboard") {
        "keyboard"
    } else if key.starts_with("mouse") || key.starts_with("touchpad") {
        "mouse"
    } else if key.starts_with("dateandtime") || key.starts_with("time") {
        "datetime"
    } else if key.starts_with("region") || key.starts_with("language") {
        "region"
    } else if key.starts_with("privacy") {
        "privacy"
    } else if key.starts_with("accounts") || key.starts_with("yourinfo") {
        "user-accounts"
    } else if key.starts_with("appsfeatures") || key.starts_with("apps") {
        "applications"
    } else if key.starts_with("windowsupdate") || key.starts_with("update") {
        let _ = Command::new("update-manager").spawn();
        return;
    } else {
        "info-overview"
    };

    let _ = Command::new("gnome-control-center")
        .arg(panel)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(target_os = "macos")]
fn dispatch_windows_uri(_uri: &str) {
    // macOS System Settings 没有稳定的 per-panel URL，
    // 直接打开总览由用户选择
    let _ = Command::new("open")
        .args(["-b", "com.apple.systempreferences"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}


// ============================================================
//  测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::config;

    /// 历史读写会影响真实的 `~/.config/run-dialog/`，故测试期间把 HOME
    /// 指向临时目录。
    ///
    /// 环境变量是进程级的，这些测试必须串行——用静态互斥锁保证。
    fn with_temp_home<T>(f: impl FnOnce() -> T) -> T {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let saved = std::env::var_os("HOME");
        let dir = std::env::temp_dir().join(format!(
            "rd-hist-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("HOME", &dir);

        let out = f();

        match saved {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn history_starts_empty() {
        with_temp_home(|| assert!(config::load_history().is_empty()));
    }

    #[test]
    fn history_roundtrip() {
        with_temp_home(|| {
            config::push_history("ls");
            config::push_history("gedit");
            assert_eq!(config::load_history(), ["ls", "gedit"]);
        });
    }

    #[test]
    fn history_skips_consecutive_duplicates() {
        with_temp_home(|| {
            config::push_history("ls");
            config::push_history("ls");
            config::push_history("ls");
            assert_eq!(config::load_history(), ["ls"], "连续重复应只记一条");

            // 非连续重复要保留
            config::push_history("gedit");
            config::push_history("ls");
            assert_eq!(config::load_history(), ["ls", "gedit", "ls"]);
        });
    }

    #[test]
    fn history_ignores_empty_input() {
        with_temp_home(|| {
            config::push_history("");
            config::push_history("   ");
            assert!(config::load_history().is_empty());
        });
    }

    #[test]
    fn history_sanitizes_newlines() {
        with_temp_home(|| {
            // 多行输入不能破坏「一行一条」的格式
            config::push_history("echo a\necho b");
            let h = config::load_history();
            assert_eq!(h.len(), 1, "含换行的输入应记为一条");
            assert!(!h[0].contains('\n'));
        });
    }

    #[test]
    fn history_trims_whitespace() {
        with_temp_home(|| {
            config::push_history("  ls  ");
            assert_eq!(config::load_history(), ["ls"]);
        });
    }

    #[test]
    fn history_respects_limit() {
        with_temp_home(|| {
            for i in 0..(config::HISTORY_LIMIT + 20) {
                config::push_history(&format!("cmd{i}"));
            }
            let h = config::load_history();
            assert_eq!(h.len(), config::HISTORY_LIMIT, "应截断到上限");
            assert_eq!(
                h.last().unwrap(),
                &format!("cmd{}", config::HISTORY_LIMIT + 19),
                "保留的应是最新的那批"
            );
        });
    }

    #[test]
    fn history_clear_works() {
        with_temp_home(|| {
            config::push_history("ls");
            assert!(!config::load_history().is_empty());
            config::clear_history().unwrap();
            assert!(config::load_history().is_empty());
        });
    }

    #[test]
    fn history_clear_is_idempotent() {
        with_temp_home(|| {
            // 文件本就不存在时也不该报错
            config::clear_history().unwrap();
            config::clear_history().unwrap();
        });
    }
}
