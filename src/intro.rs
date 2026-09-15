//! 首次运行引导（`run-dialog intro`）。
//!
//! 用 libadwaita 的 `Carousel` 做多页向导：欢迎 → 偏好设置 → 完成。
//! 只手动触发（`run-dialog intro`），不做「配置不存在时自动弹出」——
//! 用户可能只是想让程序安静地跑起来，突然弹窗会显得冒犯。
//!
//! 偏好项直接读写 [`crate::config::Config`]，与设置窗口共用同一份配置，
//! 因此两边看到的状态始终一致。

use adw::prelude::*;
use libadwaita as adw;

use crate::i18n::{t, tf};

/// 引导页数，用于「第 N 步 / 共 M 步」与末页判定。
const PAGE_COUNT: u32 = 3;

pub fn build_intro_window(app: &gtk::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(&t("Welcome to Run"))
        .default_width(560)
        .default_height(520)
        // 引导过程会写配置，禁止拉得过小导致按钮挤在一起
        .resizable(false)
        .build();

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    // 引导窗口不提供最小化等常规操作，标题栏保持简洁
    toolbar.add_top_bar(&header);

    // ---------- 内容轮播 ----------
    let carousel = adw::Carousel::new();
    carousel.set_allow_mouse_drag(false); // 拖动切页会跳过必读信息
    carousel.set_allow_scroll_wheel(false);
    carousel.set_vexpand(true);

    carousel.append(&build_welcome_page());
    carousel.append(&build_preferences_page());
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
    // 首页时「返回」无意义，末页时「下一步」变为「完成」
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

    // Esc 关闭：libadwaita 的默认行为，这里显式说明依赖它
    toolbar.set_content(Some(&carousel));
    window.set_content(Some(&toolbar));
    window.present();
}

/// 第 1 页：欢迎 + 说明能做什么。
fn build_welcome_page() -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 18);
    box_.set_valign(gtk::Align::Center);
    box_.set_margin_start(36);
    box_.set_margin_end(36);

    let icon = gtk::Image::from_icon_name("system-run-symbolic");
    icon.set_pixel_size(96);
    icon.add_css_class("dim-label");
    box_.append(&icon);

    let title = gtk::Label::new(Some(&t("Welcome to Run")));
    title.add_css_class("title-1");
    box_.append(&title);

    let subtitle = gtk::Label::new(Some(&t(
        "Type a command, a path or a web address, then press Enter.",
    )));
    subtitle.add_css_class("dim-label");
    subtitle.set_wrap(true);
    subtitle.set_justify(gtk::Justification::Center);
    box_.append(&subtitle);

    let hint = gtk::Label::new(Some(&t(
        "The next page covers a couple of options. You can change them later in Settings.",
    )));
    hint.add_css_class("dim-label");
    hint.set_wrap(true);
    hint.set_justify(gtk::Justification::Center);
    box_.append(&hint);

    box_.upcast()
}

/// 第 2 页：偏好设置（两个开关，直接写入 config.ini）。
fn build_preferences_page() -> gtk::Widget {
    let cfg = crate::config::Config::load();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.set_valign(gtk::Align::Center);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let title = gtk::Label::new(Some(&t("Choose your preferences")));
    title.add_css_class("title-2");
    title.set_halign(gtk::Align::Start);
    content.append(&title);

    let group = adw::PreferencesGroup::new();

    // ---- Windows 兼容层 ----
    let compat_row = adw::SwitchRow::builder()
        .title(&t("Enable Windows path translation"))
        .subtitle(&t(
            "Map C:\\ paths to their Linux counterparts and translate common .exe names",
        ))
        .active(cfg.enable_win_compat)
        .build();
    compat_row.connect_active_notify(|row| save_config(|c| c.enable_win_compat = row.is_active()));
    group.add(&compat_row);

    // ---- 多候选选择 ----
    let pick_row = adw::SwitchRow::builder()
        .title(&t("Choose when several apps match"))
        .subtitle(&t(
            "Show a list to pick from instead of launching the first match",
        ))
        .active(cfg.pick_desktop)
        .build();
    pick_row.connect_active_notify(|row| save_config(|c| c.pick_desktop = row.is_active()));
    group.add(&pick_row);

    content.append(&group);

    let note = gtk::Label::new(Some(&t("Both options are off by default, matching the Windows Run box.")));
    note.add_css_class("dim-label");
    note.set_wrap(true);
    note.set_justify(gtk::Justification::Center);
    content.append(&note);

    content.upcast()
}

/// 第 3 页：完成，提示后续入口。
fn build_done_page() -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 18);
    box_.set_valign(gtk::Align::Center);
    box_.set_margin_start(36);
    box_.set_margin_end(36);

    let icon = gtk::Image::from_icon_name("object-select-symbolic");
    icon.set_pixel_size(96);
    icon.add_css_class("success");
    box_.append(&icon);

    let title = gtk::Label::new(Some(&t("You are all set")));
    title.add_css_class("title-1");
    box_.append(&title);

    let subtitle = gtk::Label::new(Some(&t(
        "Press Super+R to open the run dialog at any time.",
    )));
    subtitle.add_css_class("dim-label");
    subtitle.set_wrap(true);
    subtitle.set_justify(gtk::Justification::Center);
    box_.append(&subtitle);

    let hint = gtk::Label::new(Some(&t("Run \"run-dialog settings\" to revisit the options.")));
    hint.add_css_class("dim-label");
    hint.set_wrap(true);
    hint.set_justify(gtk::Justification::Center);
    box_.append(&hint);

    box_.upcast()
}

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
