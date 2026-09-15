//! 首次运行引导（`run-dialog intro`）。
//!
//! 用 libadwaita 的 `Carousel` 做多页向导：
//!
//!   1. 欢迎
//!   2. 为什么用它（相比 GNOME 自带运行窗口的优势）
//!   3. 偏好选项（Windows 兼容层 / 多候选选择）
//!   4. 快捷键（Super+R，含冲突检测）
//!   5. 外观（跟随系统 / 浅色 / 深色）
//!   6. 语言（说明生效条件，并检测当前环境）
//!   7. 完成
//!
//! 只手动触发（`run-dialog intro`）。安装脚本 `scripts/install.sh` 完成后
//! 会询问是否调起本向导，从而把「装文件」与「设偏好」两件事分开。
//!
//! 偏好项直接读写 [`crate::config::Config`] 或 gsettings，
//! 与设置窗口共用同一份状态，因此两边看到的结果始终一致。

use adw::prelude::*;
use libadwaita as adw;

use crate::i18n::{t, tf, N_};

/// 引导页数，用于末页判定。
const PAGE_COUNT: u32 = 7;

// ============================================================
//  GNOME 快捷键（gsettings）常量
// ============================================================

const KEYBINDING_PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/run-dialog/";
const MEDIA_KEYS_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const CUSTOM_SCHEMA_PREFIX: &str =
    "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:";

/// 外观页的三个选项。
///
/// 注意与 `adw::ColorScheme` 的区别：libadwaita 还有 `ForceLight` / `ForceDark`，
/// 那两个会**覆盖**应用的明暗选择；这里只用「跟随系统 / 偏好浅色 / 偏好深色」三种，
/// 与 GNOME 设置面板里的选项一致。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorScheme {
    Default,
    Light,
    Dark,
}

impl ColorScheme {
    /// 读当前值 —— 从 `StyleManager` 读，而不是读 dconf。
    ///
    /// 两者在正常情况下一一致，但 `StyleManager` 反映的是**实际生效**的方案，
    /// 而 dconf 里可能是别的值（例如被环境变量覆盖时）。
    fn current() -> Self {
        match adw::StyleManager::default().color_scheme() {
            adw::ColorScheme::PreferLight => Self::Light,
            adw::ColorScheme::PreferDark => Self::Dark,
            _ => Self::Default,
        }
    }

    fn to_adw(&self) -> adw::ColorScheme {
        match self {
            Self::Default => adw::ColorScheme::Default,
            Self::Light => adw::ColorScheme::PreferLight,
            Self::Dark => adw::ColorScheme::PreferDark,
        }
    }
}

pub fn build_intro_window(app: &gtk::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(&t("Welcome to Run"))
        .default_width(620)
        .default_height(620)
        // 引导过程会写配置，禁止拉得过小导致按钮挤在一起
        .resizable(false)
        .build();

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar.add_top_bar(&header);

    // ---------- 内容轮播 ----------
    let carousel = adw::Carousel::new();
    carousel.set_allow_mouse_drag(false); // 拖动切页会跳过必读信息
    carousel.set_allow_scroll_wheel(false);
    carousel.set_vexpand(true);

    carousel.append(&build_welcome_page());
    carousel.append(&build_why_page());
    carousel.append(&build_preferences_page());
    carousel.append(&build_shortcut_page());
    carousel.append(&build_appearance_page());
    carousel.append(&build_language_page());
    carousel.append(&build_done_page());

    // ---------- 底部：页码指示 + 按钮 ----------
    let dots = adw::CarouselIndicatorDots::new();
    dots.set_carousel(Some(&carousel));

    let back_btn = gtk::Button::with_label(&t("Back"));
    let next_btn = gtk::Button::with_label(&t("Next"));
    next_btn.add_css_class("suggested-action");

    let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    button_box.set_halign(gtk::Align::Center);
    button_box.append(&back_btn);
    button_box.append(&next_btn);

    let bottom = gtk::Box::new(gtk::Orientation::Vertical, 12);
    bottom.set_margin_top(12);
    bottom.set_margin_bottom(18);
    bottom.set_margin_start(12);
    bottom.set_margin_end(12);
    bottom.append(&dots);
    bottom.append(&button_box);
    toolbar.add_bottom_bar(&bottom);

    // ---------- 状态同步 ----------
    let sync_buttons = {
        let carousel = carousel.clone();
        let back_btn = back_btn.clone();
        let next_btn = next_btn.clone();

        move || {
            let idx = carousel.position().round() as u32;
            back_btn.set_visible(idx > 0);

            if idx + 1 >= PAGE_COUNT {
                next_btn.set_label(&t("Done"));
            } else {
                next_btn.set_label(&t("Next"));
            }
        }
    };

    {
        let sync_buttons = sync_buttons.clone();
        carousel.connect_page_changed(move |_, _| sync_buttons());
    }
    sync_buttons(); // 初始状态

    back_btn.connect_clicked({
        let carousel = carousel.clone();
        move |_| {
            let prev = (carousel.position().round() as u32).saturating_sub(1);
            carousel.scroll_to(&carousel.nth_page(prev), true);
        }
    });

    next_btn.connect_clicked({
        let carousel = carousel.clone();
        let window = window.clone();
        move |_| {
            let idx = carousel.position().round() as u32;
            if idx + 1 >= PAGE_COUNT {
                window.close();
            } else {
                carousel.scroll_to(&carousel.nth_page(idx + 1), true);
            }
        }
    });

    toolbar.set_content(Some(&carousel));
    window.set_content(Some(&toolbar));
    window.present();
}

// ============================================================
//  页面构件
// ============================================================

/// 统一的页面容器：垂直居中 + 左右留白。
fn page_box(spacing: i32) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, spacing);
    b.set_valign(gtk::Align::Center);
    b.set_margin_start(36);
    b.set_margin_end(36);
    b
}

/// 大号图标 + 标题的页首。
fn page_heading(icon_name: &str, title: &str, icon_class: Option<&str>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 12);
    b.set_halign(gtk::Align::Center);

    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(88);
    icon.add_css_class(icon_class.unwrap_or("dim-label"));
    b.append(&icon);

    let label = gtk::Label::new(Some(title));
    label.add_css_class("title-1");
    label.set_wrap(true);
    label.set_justify(gtk::Justification::Center);
    b.append(&label);

    b
}

/// 居中的次要说明文字。
fn dim_label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("dim-label");
    l.set_wrap(true);
    l.set_justify(gtk::Justification::Center);
    l
}

// ============================================================
//  第 1 页：欢迎
// ============================================================

fn build_welcome_page() -> gtk::Widget {
    let b = page_box(18);
    b.append(&page_heading(
        "system-run-symbolic",
        &t("Welcome to Run"),
        None,
    ));
    b.append(&dim_label(&t(
        "Type a command, a path or a web address, then press Enter.",
    )));
    b.append(&dim_label(&t(
        "The next few pages cover the shortcuts, appearance and a couple of options.",
    )));
    b.upcast()
}

// ============================================================
//  第 2 页：为什么用它
// ============================================================

fn build_why_page() -> gtk::Widget {
    let b = page_box(18);
    b.append(&page_heading(
        "starred-symbolic",
        &t("Why use this instead?"),
        Some("accent"),
    ));

    let group = adw::PreferencesGroup::new();
    group.set_title(&t("Compared with the GNOME run dialog"));

    // 每条优势用 ActionRow 表达：标题给结论，副标题给原因。
    //
    // 字符串先存进数组、之后才经变量传给 t()，xgettext 无法识别，
    // 因此用 N_() 标记（展开为原串，见 i18n::N_）。
    let rows: [(&str, &str); 4] = [
        (
            N_("Finds commands, files, folders and apps in one box"),
            N_("A single lookup order covers executables, paths, desktop entries and URLs, so you never have to think about which one you are typing."),
        ),
        (
            N_("Never goes through a shell"),
            N_("Arguments are passed as an argument vector, so wildcards, pipes and variables are not silently expanded. This avoids injection surprises."),
        ),
        (
            N_("Opens a terminal only when the program needs one"),
            N_("Interpreters such as python or gdb are detected and wrapped in a terminal automatically; ls or cat are left alone."),
        ),
        (
            N_("Runs programs as administrator, UAC style"),
            N_("A consent dialog collects your password and hands it to sudo's askpass helper. The password is wiped from memory as soon as it is used."),
        ),
    ];

    for (title, subtitle) in rows {
        let row = adw::ActionRow::builder()
            .title(&t(title))
            .subtitle(&t(subtitle))
            .build();
        group.add(&row);
    }

    b.append(&group);
    b.upcast()
}

// ============================================================
//  第 3 页：偏好选项
// ============================================================

fn build_preferences_page() -> gtk::Widget {
    let cfg = crate::config::Config::load();

    let b = page_box(18);
    b.append(&page_heading(
        "preferences-system-symbolic",
        &t("Choose your preferences"),
        None,
    ));

    let group = adw::PreferencesGroup::new();

    let compat_row = adw::SwitchRow::builder()
        .title(&t("Enable Windows path translation"))
        .subtitle(&t(
            "Map C:\\ paths to their Linux counterparts and translate common .exe names",
        ))
        .active(cfg.enable_win_compat)
        .build();
    compat_row.connect_active_notify(|row| save_config(|c| c.enable_win_compat = row.is_active()));
    group.add(&compat_row);

    let pick_row = adw::SwitchRow::builder()
        .title(&t("Choose when several apps match"))
        .subtitle(&t(
            "Show a list to pick from instead of launching the first match",
        ))
        .active(cfg.pick_desktop)
        .build();
    pick_row.connect_active_notify(|row| save_config(|c| c.pick_desktop = row.is_active()));
    group.add(&pick_row);

    b.append(&group);
    b.append(&dim_label(&t(
        "Both options are off by default, matching the Windows Run box.",
    )));
    b.upcast()
}

// ============================================================
//  第 4 页：快捷键
// ============================================================

fn build_shortcut_page() -> gtk::Widget {
    let b = page_box(18);
    b.append(&page_heading(
        "preferences-desktop-keyboard-shortcuts-symbolic",
        &t("Set up a shortcut"),
        None,
    ));

    let group = adw::PreferencesGroup::new();
    group.set_title(&t("Keyboard shortcut"));

    // 先探测状态，用于决定初始文案
    let occupied_by = super_r_taken_by();
    let already_ours = own_binding_registered();

    let status = if already_ours {
        t("Super+R is already set up for Run.")
    } else if let Some(ref owner) = occupied_by {
        // msgid 保持纯 ASCII：xgettext 会把源码里的非 ASCII 字符
        // 转义成 \u{....} 写进模板，翻译对不上。引号交给译文处理。
        tf("{1} currently uses Super+R.", &[owner])
    } else {
        t("Super+R is free.")
    };

    let row = adw::ActionRow::builder()
        .title(&t("Open Run with Super+R"))
        .subtitle(&status)
        .build();

    let btn = gtk::Button::with_label(&t("Set Up"));
    btn.set_valign(gtk::Align::Center);
    if already_ours {
        btn.set_sensitive(false);
    }
    row.add_suffix(&btn);
    row.set_activatable_widget(Some(&btn));

    // 点击后更新副标题，给出即时反馈
    let row_clone = row.clone();
    let btn_clone = btn.clone();
    btn.connect_clicked(move |_| match register_shortcut() {
        Ok(()) => {
            row_clone.set_subtitle(&t("Super+R is already set up for Run."));
            btn_clone.set_sensitive(false);
        }
        Err(e) => {
            row_clone.set_subtitle(&tf("Could not set up the shortcut: {1}", &[&e]));
        }
    });

    group.add(&row);
    b.append(&group);

    // 若被占用，提示会发生什么
    if occupied_by.is_some() && !already_ours {
        b.append(&dim_label(&t(
            "Setting it up will override the shortcut that uses it now.",
        )));
    }

    b.upcast()
}

// ============================================================
//  第 5 页：外观
// ============================================================

fn build_appearance_page() -> gtk::Widget {
    let active = ColorScheme::current();

    let b = page_box(18);
    b.append(&page_heading(
        "preferences-desktop-theme-symbolic",
        &t("Pick an appearance"),
        None,
    ));

    let group = adw::PreferencesGroup::new();
    group.set_title(&t("Interface style"));

    // 用 ActionRow + 单选按钮，比下拉列表更直观
    let mut first: Option<gtk::CheckButton> = None;
    for (scheme, title, subtitle, icon) in [
        (
            ColorScheme::Default,
            N_("Follow the system"),
            N_("Use whatever the desktop is already set to"),
            "display-symbolic",
        ),
        (
            ColorScheme::Light,
            N_("Light"),
            N_("Always use the light style"),
            "weather-clear-symbolic",
        ),
        (
            ColorScheme::Dark,
            N_("Dark"),
            N_("Always use the dark style"),
            "weather-clear-night-symbolic",
        ),
    ] {
        let row = adw::ActionRow::builder()
            .title(&t(title))
            .subtitle(&t(subtitle))
            .build();

        let check = gtk::CheckButton::new();
        check.set_valign(gtk::Align::Center);
        // 同一个「组」内互斥；第一个创建的作为组首
        match &first {
            None => first = Some(check.clone()),
            Some(f) => check.set_group(Some(f)),
        }
        check.set_active(scheme == active);
        row.add_prefix(&gtk::Image::from_icon_name(icon));
        row.add_suffix(&check);
        row.set_activatable_widget(Some(&check));

        check.connect_toggled(move |c| {
            if c.is_active() {
                // 必须走 libadwaita 的 StyleManager，**不能**直接 `gsettings set`。
                //
                // 直接写 dconf 会让 GTK 异步重载主题；若主题资源加载失败
                // （例如 Yaru-dark 的 GResource 缺失），CSS provider 会损坏，
                // 后果是窗口背景连同标题栏一起变成全透明，且切回去也修不好，
                // 只能重启程序。
                //
                // StyleManager 走的是 libadwaita 的样式栈，会优雅地重建
                // CSS，不依赖系统主题资源是否完好。
                adw::StyleManager::default().set_color_scheme(scheme.to_adw());
            }
        });

        group.add(&row);
    }

    b.append(&group);
    b.append(&dim_label(&t(
        "This changes the appearance for the whole desktop, not just Run.",
    )));
    b.upcast()
}

// ============================================================
//  第 6 页：语言
// ============================================================

fn build_language_page() -> gtk::Widget {
    let b = page_box(18);
    b.append(&page_heading(
        "preferences-desktop-locale-symbolic",
        &t("Interface language"),
        None,
    ));

    let group = adw::PreferencesGroup::new();
    group.set_title(&t("How the language is chosen"));

    // 环境检测：LANG 是否合法、译文是否已安装
    let lang_var = std::env::var("LANG").unwrap_or_else(|_| t("(not set)"));
    let valid_locale = is_valid_locale_name(&lang_var);
    let catalog = find_catalog();

    let rows: [(&str, String); 3] = [
        (N_("Current system language"), lang_var),
        (
            N_("Locale name"),
            if valid_locale {
                t("Looks valid.")
            } else {
                t("This is not a valid locale name, so English is used instead.")
            },
        ),
        (
            N_("Translation installed"),
            match catalog {
                Some(ref p) => tf("Found at {1}", &[p]),
                None => t("Not found. Run the installer to copy it."),
            },
        ),
    ];

    for (title, subtitle) in rows {
        let row = adw::ActionRow::builder()
            .title(&t(title))
            .subtitle(&subtitle)
            .build();
        group.add(&row);
    }

    b.append(&group);
    b.append(&dim_label(&t(
        "Run decides its language from the desktop locale. It cannot be switched inside the program.",
    )));
    b.upcast()
}

// ============================================================
//  第 7 页：完成
// ============================================================

fn build_done_page() -> gtk::Widget {
    let b = page_box(18);
    b.append(&page_heading(
        "object-select-symbolic",
        &t("You are all set"),
        Some("success"),
    ));
    b.append(&dim_label(&t(
        "Press Super+R to open the run dialog at any time.",
    )));
    b.append(&dim_label(&t(
        "Run \"run-dialog settings\" to revisit the options.",
    )));
    b.upcast()
}

// ============================================================
//  辅助函数
// ============================================================

/// 读取 → 修改 → 写回。
///
/// 每次重新 `load()` 而不是持有 `Config`，
/// 避免与其他窗口（设置窗口）同时修改时互相覆盖。
fn save_config(f: impl FnOnce(&mut crate::config::Config)) {
    let mut cfg = crate::config::Config::load();
    f(&mut cfg);
    if let Err(e) = cfg.save() {
        eprintln!("{}", tf("Failed to save settings: {1}", &[&e.to_string()]));
    }
}

// ---------------- gsettings ----------------

fn gsettings_get(schema: &str, key: &str) -> Option<String> {
    let out = std::process::Command::new("gsettings")
        .args(["get", schema, key])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn gsettings_set(schema: &str, key: &str, value: &str) -> Result<(), String> {
    let out = std::process::Command::new("gsettings")
        .args(["set", schema, key, value])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

// ---------------- 快捷键 ----------------

fn custom_schema() -> String {
    format!("{CUSTOM_SCHEMA_PREFIX}{KEYBINDING_PATH}")
}

/// 当前所有自定义快捷键的路径列表。
fn current_bindings() -> Vec<String> {
    let raw = gsettings_get(MEDIA_KEYS_SCHEMA, "custom-keybindings").unwrap_or_default();
    raw.trim_matches(|c| c == '[' || c == ']')
        .split(',')
        .map(|s| s.trim().trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 我们已经注册过？
fn own_binding_registered() -> bool {
    current_bindings().iter().any(|p| p == KEYBINDING_PATH)
}

/// Super+R 是否被别的自定义快捷键占用；返回占用者名字。
fn super_r_taken_by() -> Option<String> {
    for path in current_bindings() {
        if path == KEYBINDING_PATH {
            continue;
        }
        let schema = format!("{CUSTOM_SCHEMA_PREFIX}{path}");
        let binding = gsettings_get(&schema, "binding")
            .unwrap_or_default()
            .trim_matches('\'')
            .to_lowercase();
        if binding == "<super>r" {
            let name = gsettings_get(&schema, "name")
                .unwrap_or_default()
                .trim_matches('\'')
                .to_string();
            return Some(if name.is_empty() {
                t("an unnamed shortcut")
            } else {
                name
            });
        }
    }
    None
}

/// 注册 Super+R；已注册则不重复添加。
fn register_shortcut() -> Result<(), String> {
    if !own_binding_registered() {
        let mut list = current_bindings();
        list.push(KEYBINDING_PATH.to_string());
        // gsettings 要求写回完整列表，不能只追加一项
        let joined = list
            .iter()
            .map(|p| format!("'{p}'"))
            .collect::<Vec<_>>()
            .join(", ");
        gsettings_set(
            MEDIA_KEYS_SCHEMA,
            "custom-keybindings",
            &format!("[{joined}]"),
        )?;
    }

    let schema = custom_schema();
    // 命令用绝对路径：GNOME 会话的 PATH 不一定包含 ~/.local/bin
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "run-dialog".to_string());

    gsettings_set(&schema, "name", &t("Show the run command prompt"))?;
    gsettings_set(&schema, "command", &exe)?;
    gsettings_set(&schema, "binding", "<Super>r")?;
    Ok(())
}

// ---------------- 语言检测 ----------------

/// 判断 LANG 是否是可用的 locale 名。
///
/// 关键点：必须带编码后缀（`zh_CN.UTF-8`）。写成 `zh_CN` 时 glibc 不认，
/// GTK 会退回 C locale，界面就变成英文了。
fn is_valid_locale_name(lang: &str) -> bool {
    let lang = lang.trim();
    // glibc 接受的写法一定带 '.'（如 zh_CN.UTF-8），
    // 或者是不带修饰的 C / POSIX。其余（如裸 "zh_CN"）会被忽略。
    matches!(lang, "C" | "POSIX" | "C.UTF-8") || lang.contains('.')
}

/// 在常见位置寻找已安装的 `.mo`，找到则返回路径。
fn find_catalog() -> Option<String> {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();

    if let Some(dir) = std::env::var_os("RUN_DIALOG_LOCALEDIR") {
        roots.push(dir.into());
    }
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        roots.push(std::path::PathBuf::from(dir).join("locale"));
    } else if let Some(home) = std::env::var_os("HOME") {
        roots.push(std::path::PathBuf::from(home).join(".local/share/locale"));
    }
    roots.push(std::path::PathBuf::from("/usr/share/locale"));

    for root in roots {
        let candidate = root.join("zh_CN/LC_MESSAGES/run-dialog.mo");
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}
