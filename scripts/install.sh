#!/usr/bin/bash
#
# run-dialog 安装向导
#
# 本脚本位于 <项目根>/scripts/，请从任意位置调用：
#     ./scripts/install.sh
#
# 只负责「把文件装到位」：
#   1. 安装文件（二进制 / 译文 / .desktop / 图标）到 ~/.local
#   2. 检查 ~/.local/bin 是否在 PATH 中
#   3. 询问是否运行 `run-dialog intro`（偏好设置交给程序内的引导向导）
#
# 分工说明：快捷键、外观、语言等偏好设置统一由 `run-dialog intro` 负责，
# 不在 shell 里重复实现——否则同一套逻辑要在两个地方各维护一遍。
#
# 设计要点：
# - **用户级安装**：全部写到 ~/.local，不需要 root
# - **幂等**：重复运行会覆盖安装，不会报错
# - **可取消**：任何一步取消都优雅退出，已完成的步骤不回滚（用户可重跑）
# - 不依赖项目源码目录以外的任何东西

set -uo pipefail

# ---------------- 基本路径 ----------------

# 本脚本位于 <项目根>/scripts/，所以项目根是它的上一级
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

BIN_SRC="$PROJECT_ROOT/target/release/run-dialog"
DESKTOP_SRC="$PROJECT_ROOT/resources/run-dialog.desktop"
ICON_SRC="$PROJECT_ROOT/resources/icon.png"
LOCALE_SRC="$PROJECT_ROOT/target/release/locale"

# 安装目标（XDG 规范）
PREFIX="${XDG_DATA_HOME:-$HOME/.local/share}"
BIN_DIR="$HOME/.local/bin"
APP_DIR="$PREFIX/applications"
LOCALE_DST="$PREFIX/locale"
ICON_DST="$PREFIX/icons/hicolor/256x256/apps"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/run-dialog"

BIN_DST="$BIN_DIR/run-dialog"
DESKTOP_DST="$APP_DIR/run-dialog.desktop"
ICON_DST_FILE="$ICON_DST/run-dialog.png"

APP_TITLE="运行 安装向导"

# ---------------- 输出辅助 ----------------

# zenity 的 info/warning/error 都是阻塞的，用户点确定才继续
info()  { zenity --info  --title="$APP_TITLE" --width=380 --text="$1" 2>/dev/null; }
warn()  { zenity --warning --title="$APP_TITLE" --width=380 --text="$1" 2>/dev/null; }
err()   { zenity --error --title="$APP_TITLE" --width=380 --text="$1" 2>/dev/null; }

# 询问用户是否继续；返回 0=是，1=否
confirm() {
    zenity --question --title="$APP_TITLE" --width=420 \
           --ok-label="继续" --cancel-label="取消" --text="$1" 2>/dev/null
}

# ---------------- 前置检查 ----------------

check_prereqs() {
    local missing=()

    command -v zenity >/dev/null 2>&1 || missing+=("zenity")
    command -v gsettings >/dev/null 2>&1 || missing+=("gsettings (glib)")

    if [[ ! -x "$BIN_SRC" ]]; then
        err "找不到已编译的程序：\n$BIN_SRC\n\n请先在项目根目录运行 ./build.sh 编译。"
        exit 1
    fi

    if (( ${#missing[@]} > 0 )); then
        err "缺少必要工具：\n${missing[*]}\n\n请安装后重试。"
        exit 1
    fi
}

# ---------------- 步骤 1：安装文件 ----------------

install_files() {
    local log
    log=$(mktemp)

    # 用进度条展示，避免感觉卡住
    (
        set -e
        echo "# 创建工作目录…"
        mkdir -p "$BIN_DIR" "$APP_DIR" "$CONFIG_DIR"

        echo "# 安装程序…"
        install -Dm755 "$BIN_SRC" "$BIN_DST"

        echo "# 安装桌面条目…"
        install -Dm644 "$DESKTOP_SRC" "$DESKTOP_DST"

        # 图标：.desktop 的 Icon=run-dialog 会按规定主题路径查找，
        # 缺失时应用菜单/停靠栏会退化成空白或通用图标。
        if [[ -f "$ICON_SRC" ]]; then
            echo "# 安装图标…"
            install -Dm644 "$ICON_SRC" "$ICON_DST_FILE"
        fi

        # 译文：这一步是「Super+R 显示英文」问题的关键。
        # 程序按顺序探测 locale 目录，~/.local/share/locale 在其中，
        # 但必须在 `<root>/<lang>/LC_MESSAGES/run-dialog.mo` 真的存在。
        if [[ -d "$LOCALE_SRC" ]]; then
            echo "# 安装译文…"
            for lang_dir in "$LOCALE_SRC"/*/; do
                [[ -d "$lang_dir" ]] || continue
                local lang
                lang="$(basename "$lang_dir")"
                if [[ -f "$lang_dir/LC_MESSAGES/run-dialog.mo" ]]; then
                    install -Dm644 \
                        "$lang_dir/LC_MESSAGES/run-dialog.mo" \
                        "$LOCALE_DST/$lang/LC_MESSAGES/run-dialog.mo"
                fi
            done
        else
            echo "# 警告：未找到译文目录，中文界面可能不可用"
            echo "# （请确认已在项目根目录运行 ./build.sh，且 po/ 下有 .po 文件）"
        fi

        echo "# 更新桌面数据库…"
        if command -v update-desktop-database >/dev/null 2>&1; then
            update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
        fi

        # 刷新图标缓存，否则新装的图标可能要重新登录才出现
        if command -v gtk4-update-icon-cache >/dev/null 2>&1; then
            gtk4-update-icon-cache -q -t -f "$PREFIX/icons/hicolor" >/dev/null 2>&1 || true
        elif command -v gtk-update-icon-cache >/dev/null 2>&1; then
            gtk-update-icon-cache -q -t -f "$PREFIX/icons/hicolor" >/dev/null 2>&1 || true
        fi

        echo "# 完成"
        echo "100"
    ) 2>&1 | zenity --progress --title="$APP_TITLE" --width=420 \
                     --auto-close --no-cancel --pulsate \
                     --text="正在安装…" >>"$log" 2>/dev/null

    rm -f "$log"

    # 校验安装结果
    if [[ ! -x "$BIN_DST" ]]; then
        err "安装失败：$BIN_DST 不存在或不可执行。"
        return 1
    fi
    return 0
}

# ---------------- 步骤 2：PATH 检查 ----------------

check_path() {
    if [[ ":$PATH:" == *":$BIN_DIR:"* ]]; then
        return 0
    fi

    # 未在 PATH 中：提示用户，并给出解决办法
    if confirm "安装目录 $BIN_DIR 不在你的 PATH 中。\n\n这会导致「打开运行」的快捷键找不到程序。\n\n是否帮你把下面这行追加到 ~/.profile？\n\nexport PATH=\"\$HOME/.local/bin:\$PATH\""; then
        local profile="$HOME/.profile"
        local line='export PATH="$HOME/.local/bin:$PATH"'
        if ! grep -qF "$line" "$profile" 2>/dev/null; then
            {
                echo ""
                echo "# 由 run-dialog 安装向导添加"
                echo "$line"
            } >> "$profile"
            info "已写入 ~/.profile。\n\n需要重新登录（或执行 source ~/.profile）后生效。"
        else
            info "~/.profile 中已存在该配置。"
        fi
    fi
}


main() {
    check_prereqs

    if ! confirm "即将把「运行」安装到你的用户目录：\n\n  程序：$BIN_DST\n  桌面条目：$DESKTOP_DST\n  图标：$ICON_DST_FILE\n  译文：$LOCALE_DST/\n  配置：$CONFIG_DIR/\n\n整个过程不需要管理员权限。是否继续？"; then
        exit 0
    fi

    if ! install_files; then
        exit 1
    fi

    check_path

    # 偏好设置（快捷键、外观、语言等）统一交给程序内的引导向导，
    # 避免在 shell 与 Rust 两侧各维护一套相同的逻辑。
    if confirm "文件已就位。\n\n是否现在运行引导向导，设置快捷键、外观与语言？\n\n（之后也可随时输入 run-dialog intro 重新打开）"; then
        # intro 会自己 present 窗口，这里不等待它退出
        setsid "$BIN_DST" intro >/dev/null 2>&1 &
    else
        info "安装完成。\n\n程序位置：$BIN_DST"
    fi
}

main "$@"
