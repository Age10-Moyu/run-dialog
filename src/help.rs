//! `run-dialog help` 的实现。
//!
//! 两种呈现方式：
//!
//! - `run-dialog help` —— 纯文本输出到 stdout，方便管道与分页（`| less`）
//! - `run-dialog help --gui` —— 图形对话框，适合从「运行」窗口里点出来
//!
//! 内容与 `resources/run-dialog.1`（man 手册）保持一致，但更紧凑：
//! man 页面向的是「查手册的人」，这里面向的是「想快速知道怎么用的人」。

use adw::prelude::*;
use libadwaita as adw;

use crate::i18n::t;

/// 程序名与版本，用于帮助头部。
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 生成帮助正文（不含标题行）。
///
/// 抽成函数是为了让终端输出与 GUI 对话框**共用同一份文案** ——
/// 两处各写一份迟早会不一致。
fn help_body() -> String {
    let mut s = String::new();

    s.push_str(&format!("{} {}\n", t("Run"), VERSION));
    s.push_str(&t("Type a command, a path or a web address, then press Enter."));
    s.push_str("\n\n");

    // ---------- 用法 ----------
    s.push_str(&format!("{}\n", t("Usage")));
    s.push_str("  run-dialog                  ");
    s.push_str(&t("Open the run dialog"));
    s.push('\n');
    s.push_str("  run-dialog settings         ");
    s.push_str(&t("Open the settings window"));
    s.push('\n');
    s.push_str("  run-dialog intro            ");
    s.push_str(&t("Run the first-time setup wizard"));
    s.push('\n');
    s.push_str("  run-dialog help [--gui]     ");
    s.push_str(&t("Show this help"));
    s.push_str("\n\n");

    // ---------- 输入解析 ----------
    s.push_str(&format!("{}\n", t("How input is resolved")));
    for (n, line) in [
        t("A web address or known URI scheme is opened by the default handler"),
        t("An explicit path is launched, or opened with the default application"),
        t("Otherwise the current directory, the system directories and $PATH are searched"),
        t("Then desktop entries from the XDG, Flatpak and Snap databases"),
        t("As a last resort the input is handed to xdg-open"),
    ]
    .iter()
    .enumerate()
    {
        s.push_str(&format!("  {}. {}\n", n + 1, line));
    }
    s.push('\n');

    // ---------- 值得知道的 ----------
    s.push_str(&format!("{}\n", t("Things worth knowing")));
    for line in [
        t("Arguments are passed as an argument vector, so wildcards, pipes and variables are not expanded"),
        t("Interpreters such as python or gdb open in a terminal automatically"),
        t("Tick the administrator checkbox to run a program with elevated privileges"),
    ] {
        s.push_str(&format!("  - {}\n", line));
    }
    s.push('\n');

    // ---------- 文件 ----------
    s.push_str(&format!("{}\n", t("Files")));
    s.push_str("  ~/.config/run-dialog/config.ini    ");
    s.push_str(&t("Settings"));
    s.push('\n');
    s.push_str("  ~/.config/run-dialog/history       ");
    s.push_str(&t("Command history"));
    s.push('\n');
    s.push_str(&t("Run \"man run-dialog\" for the full manual."));
    s.push('\n');

    s
}

/// 终端模式：打印到 stdout。
pub fn print_help() {
    print!("{}", help_body());
}

/// GUI 模式：用对话框显示。
///
/// 正文用等宽字体 —— 里面全是缩进对齐的内容，比例字体下会散架。
pub fn show_help_dialog(app: &gtk::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(&t("Help"))
        .default_width(640)
        .default_height(560)
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());

    let label = gtk::Label::new(Some(&help_body()));
    // 等宽字体 + 左对齐：正文靠空格对齐，必须用等宽
    label.set_xalign(0.0);
    label.set_yalign(0.0);
    label.set_wrap(false); // 不折行，太长的行用横向滚动处理
    label.set_selectable(true); // 允许复制，方便贴到终端里试
    label.add_css_class("monospace");
    label.set_margin_top(18);
    label.set_margin_bottom(18);
    label.set_margin_start(18);
    label.set_margin_end(18);

    // 横向滚动兜底：窄窗口下等宽文本会超出
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&label));
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);

    // 关闭按钮放在底部，比只靠标题栏更明确
    let close_btn = gtk::Button::with_label(&t("Close"));
    close_btn.add_css_class("suggested-action");
    close_btn.set_halign(gtk::Align::Center);
    close_btn.set_margin_bottom(18);
    {
        let window = window.clone();
        close_btn.connect_clicked(move |_| window.close());
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&scroll);
    content.append(&close_btn);

    toolbar.set_content(Some(&content));
    window.set_content(Some(&toolbar));
    window.present();
}
