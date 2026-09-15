use adw::prelude::*;
use libadwaita as adw;

use crate::i18n::t;

pub fn build_settings_window(app: &gtk::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(&t("Run Settings"))
        .default_width(820)
        .default_height(560)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let view_stack = adw::ViewStack::new();
    let view_switcher = adw::ViewSwitcher::builder()
        .stack(&view_stack)
        .policy(adw::ViewSwitcherPolicy::Wide)
        .build();
    header.set_title_widget(Some(&view_switcher));

    // ---------- 实验页 ----------
    let exp_page = build_exp_page();
    view_stack.add_titled_with_icon(
        &exp_page,
        Some("exp"),
        &t("Experimental"),
        "applications-science-symbolic",
    );

    // ---------- 关于页 ----------
    let about_page = build_about_page();
    view_stack.add_titled_with_icon(
        &about_page,
        Some("about"),
        &t("About"),
        "help-about-symbolic",
    );

    toolbar_view.set_content(Some(&view_stack));
    window.set_content(Some(&toolbar_view));
    window.present();
}

fn build_exp_page() -> gtk::ScrolledWindow {
    use gtk::{Orientation, ScrolledWindow};

    let content = gtk::Box::new(Orientation::Vertical, 24);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let exp_group = adw::PreferencesGroup::new();
    exp_group.set_title(&t("Windows compatibility layer"));
    exp_group.set_description(Some(&t(
        "Emulates the path and command translation of the Windows Run box. Off by default, experimental only.",
    )));

    let cfg = crate::config::Config::load();
    let switch_row = adw::SwitchRow::builder()
        .title(&t("Enable Windows path translation"))
        .subtitle(&t(
            "Map C:\\ paths to their Linux counterparts and translate common .exe names",
        ))
        .active(cfg.enable_win_compat)
        .build();

    switch_row.connect_active_notify(|row| {
        let mut cfg = crate::config::Config::load();
        cfg.enable_win_compat = row.is_active();
        if let Err(e) = cfg.save() {
            eprintln!(
                "{}",
                crate::i18n::tf("Failed to save settings: {1}", &[&e.to_string()])
            );
        }
    });

    exp_group.add(&switch_row);
    content.append(&exp_group);

    // ---------- 行为组 ----------
    let behavior_group = adw::PreferencesGroup::new();
    behavior_group.set_title(&t("Behavior"));

    let pick_row = adw::SwitchRow::builder()
        .title(&t("Choose when several apps match"))
        .subtitle(&t(
            "Show a list to pick from instead of launching the first match",
        ))
        .active(cfg.pick_desktop)
        .build();

    pick_row.connect_active_notify(|row| {
        let mut cfg = crate::config::Config::load();
        cfg.pick_desktop = row.is_active();
        if let Err(e) = cfg.save() {
            eprintln!(
                "{}",
                crate::i18n::tf("Failed to save settings: {1}", &[&e.to_string()])
            );
        }
    });

    behavior_group.add(&pick_row);
    content.append(&behavior_group);

    // ---------- 重置组 ----------
    {
        let reset_group = adw::PreferencesGroup::new();

        let reset_row = adw::ActionRow::builder()
            .title(&t("Restore default settings"))
            .subtitle(&t(
                "Turn off the Windows compatibility layer, use the first match, and clear command history",
            ))
            .build();

        let reset_btn = gtk::Button::with_label(&t("Restore Defaults"));
        reset_btn.set_valign(gtk::Align::Center);
        reset_btn.add_css_class("destructive-action");
        reset_row.add_suffix(&reset_btn);
        reset_row.set_activatable_widget(Some(&reset_btn));

        {
            let switch_row = switch_row.clone();
            let pick_row = pick_row.clone();
            reset_btn.connect_clicked(move |_| {
                // 写回默认配置
                let defaults = crate::config::Config::default();
                if let Err(e) = defaults.save() {
                    eprintln!(
                        "{}",
                        crate::i18n::tf("Failed to save settings: {1}", &[&e.to_string()])
                    );
                    return;
                }
                // 顺带清空命令历史——它同样属于「用户数据」，
                // 既然用户点了「恢复默认」，历史留着会显得没生效
                let _ = crate::config::clear_history();

                // 关键：同步界面状态。否则开关仍显示旧值，
                // 与刚写盘的配置不一致，用户会以为没生效。
                switch_row.set_active(defaults.enable_win_compat);
                pick_row.set_active(defaults.pick_desktop);
            });
        }

        reset_group.add(&reset_row);
        content.append(&reset_group);
    }

    let clamp = adw::Clamp::builder()
        .maximum_size(1000)
        .tightening_threshold(800)
        .child(&content)
        .build();

    let scroll = ScrolledWindow::new();
    scroll.set_child(Some(&clamp));
    scroll.set_vexpand(true);
    scroll
}

fn build_about_page() -> gtk::ScrolledWindow {
    use gtk::{Orientation, ScrolledWindow};

    const VERSION: &str = env!("CARGO_PKG_VERSION");

    let content = gtk::Box::new(Orientation::Vertical, 24);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let about_group = adw::PreferencesGroup::new();
    about_group.set_title(&t("Run"));

    let version_row = adw::ActionRow::builder()
        .title(&t("Version"))
        .subtitle(VERSION)
        .build();
    about_group.add(&version_row);

    let desc_row = adw::ActionRow::builder()
        .title(&t("Description"))
        .subtitle(&t("A GNOME-style run dialog"))
        .build();
    about_group.add(&desc_row);

    let app_id_row = adw::ActionRow::builder()
        .title(&t("Application ID"))
        .subtitle(crate::APP_ID)
        .build();
    about_group.add(&app_id_row);

    content.append(&about_group);

    let clamp = adw::Clamp::builder()
        .maximum_size(1000)
        .tightening_threshold(800)
        .child(&content)
        .build();

    let scroll = ScrolledWindow::new();
    scroll.set_child(Some(&clamp));
    scroll.set_vexpand(true);
    scroll
}
