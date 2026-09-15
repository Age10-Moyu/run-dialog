#!/usr/bin/bash
#
# run-dialog 卸载脚本
#
# 移除安装向导装下的所有文件，并可选地注销快捷键、清除配置。
# 与 install 对应，同样使用 zenity 交互。

set -uo pipefail

PREFIX="${XDG_DATA_HOME:-$HOME/.local/share}"
BIN_DIR="$HOME/.local/bin"
APP_DIR="$PREFIX/applications"
LOCALE_DST="$PREFIX/locale"
ICON_DST="$PREFIX/icons/hicolor/256x256/apps"
MAN_DST="$PREFIX/man/man1"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/run-dialog"

BIN_DST="$BIN_DIR/run-dialog"
DESKTOP_DST="$APP_DIR/run-dialog.desktop"
ICON_DST_FILE="$ICON_DST/run-dialog.png"
MAN_DST_FILE="$MAN_DST/run-dialog.1"

APP_TITLE="运行 卸载"

KEYBINDING_PATH="/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/run-dialog/"
SCHEMA="org.gnome.settings-daemon.plugins.media-keys"

info() { zenity --info --title="$APP_TITLE" --width=380 --text="$1" 2>/dev/null; }
err()  { zenity --error --title="$APP_TITLE" --width=380 --text="$1" 2>/dev/null; }
confirm() {
    zenity --question --title="$APP_TITLE" --width=440 \
           --ok-label="继续" --cancel-label="取消" --text="$1" 2>/dev/null
}

# 从快捷键列表中移除我们的条目
remove_shortcut() {
    local list
    list="$(gsettings get "$SCHEMA" custom-keybindings 2>/dev/null)" || return 0

    if ! echo "$list" | grep -qF "$KEYBINDING_PATH"; then
        return 0
    fi

    # 拆成条目，过滤掉我们的，再拼回去
    local items
    items="$(echo "$list" | tr -d "[]'" | tr ',' '\n' | sed '/^$/d' | grep -vF "$KEYBINDING_PATH")"

    if [[ -z "$items" ]]; then
        gsettings set "$SCHEMA" custom-keybindings "[]" 2>/dev/null
    else
        local joined
        joined="$(echo "$items" | tr '\n' ',' | sed 's/,$//')"
        gsettings set "$SCHEMA" custom-keybindings "[$joined]" 2>/dev/null
    fi
}

main() {
    # 先统计要删什么，让用户知道影响范围
    local found=()
    [[ -f "$BIN_DST" ]]       && found+=("程序:      $BIN_DST")
    [[ -f "$DESKTOP_DST" ]]   && found+=("桌面条目:  $DESKTOP_DST")
    [[ -f "$ICON_DST_FILE" ]] && found+=("图标:      $ICON_DST_FILE")
    [[ -f "$MAN_DST_FILE" ]]  && found+=("手册:      $MAN_DST_FILE")
    [[ -d "$LOCALE_DST" ]]    && found+=("译文:      $LOCALE_DST/*/LC_MESSAGES/run-dialog.mo")
    [[ -d "$CONFIG_DIR" ]]    && found+=("配置:      $CONFIG_DIR/")

    if (( ${#found[@]} == 0 )); then
        info "未发现已安装的文件。"
        exit 0
    fi

    local listing
    listing="$(printf '  %s\n' "${found[@]}")"

    if ! confirm "将从你的用户目录移除：\n\n$listing\n\n是否继续？"; then
        exit 0
    fi

    local remove_config=1
    if [[ -d "$CONFIG_DIR" ]]; then
        if ! confirm "是否同时删除配置与命令历史？\n\n$CONFIG_DIR/\n\n选择「取消」将保留它们（重新安装后设置仍在）。"; then
            remove_config=0
        fi
    fi

    # 执行删除
    rm -f "$BIN_DST" "$DESKTOP_DST" "$ICON_DST_FILE" "$MAN_DST_FILE"
    rm -rf "$LOCALE_DST"/*/LC_MESSAGES/run-dialog.mo 2>/dev/null
    if (( remove_config == 1 )); then
        rm -rf "$CONFIG_DIR"
    fi

    # 注销快捷键
    remove_shortcut

    # 更新桌面数据库
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
    fi

    # 刷新图标缓存
    if command -v gtk4-update-icon-cache >/dev/null 2>&1; then
        gtk4-update-icon-cache -q -t -f "$PREFIX/icons/hicolor" >/dev/null 2>&1 || true
    elif command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -q -t -f "$PREFIX/icons/hicolor" >/dev/null 2>&1 || true
    fi

    # 重建 man 索引，否则 apropos 仍会列出已删掉的手册
    if command -v mandb >/dev/null 2>&1; then
        mandb -q "$PREFIX/man" >/dev/null 2>&1 || true
    fi

    local msg="卸载完成。"
    if (( remove_config == 0 )); then
        msg="$msg\n\n配置与历史已保留在：\n$CONFIG_DIR/"
    fi
    msg="$msg\n\n提示：~/.profile 中的 PATH 设置未被修改，如需清理请手动编辑。"
    info "$msg"
}

main "$@"
