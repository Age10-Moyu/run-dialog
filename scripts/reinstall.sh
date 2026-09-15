#!/usr/bin/bash
#
# run-dialog 重新安装
#
# 相当于「卸载 + 安装」，但**保留配置与命令历史**，
# 且只在开头问一次 —— 不会像 uninstall.sh + install.sh 那样问两轮。
#
#     ./scripts/reinstall.sh
#
# 适用场景：改完代码想立刻看到效果，不想每次重设偏好。
#
# 与另两个脚本的关系：
#   - install.sh    装文件 + PATH 检查
#   - uninstall.sh  卸干净（可选保留配置）
#   - 本脚本        删旧文件（保留配置）→ 调 install.sh

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

APP_TITLE="运行 重新安装"

info() { zenity --info --title="$APP_TITLE" --width=420 --text="$1" 2>/dev/null; }
err()  { zenity --error --title="$APP_TITLE" --width=420 --text="$1" 2>/dev/null; }
confirm() {
    zenity --question --title="$APP_TITLE" --width=460 \
           --ok-label="继续" --cancel-label="取消" --text="$1" 2>/dev/null
}

if ! command -v zenity >/dev/null 2>&1; then
    echo "缺少 zenity，无法运行图形向导。" >&2
    echo "Debian/Ubuntu: sudo apt install zenity" >&2
    exit 1
fi

# ---------------- 路径（与 install/uninstall 保持一致）----------------

PREFIX="${XDG_DATA_HOME:-$HOME/.local/share}"
BIN_DIR="$HOME/.local/bin"
APP_DIR="$PREFIX/applications"
LOCALE_DST="$PREFIX/locale"
ICON_DST="$PREFIX/icons/hicolor/256x256/apps"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/run-dialog"

BIN_DST="$BIN_DIR/run-dialog"
DESKTOP_DST="$APP_DIR/run-dialog.desktop"
ICON_DST_FILE="$ICON_DST/run-dialog.png"

# ---------------- 1. 先编译，避免删完才失败 ----------------

# 顺序很重要：如果先删再编译失败，用户就落得「旧的没了、新的没装上」。
# 所以先确保编译产物存在，再动已安装的文件。
if [[ ! -x "$PROJECT_ROOT/target/release/run-dialog" ]]; then
    if ! confirm "尚未编译（找不到 target/release/run-dialog）。\n\n是否现在运行 ./scripts/build.sh 编译？"; then
        exit 0
    fi
    if ! "$SCRIPT_DIR/build.sh"; then
        err "编译失败，未做任何改动。\n\n请手动运行 ./scripts/build.sh 查看错误。"
        exit 1
    fi
fi

# ---------------- 2. 确认 ----------------

listing=""
[[ -f "$BIN_DST" ]]       && listing="$listing  程序：$BIN_DST\n"
[[ -f "$DESKTOP_DST" ]]   && listing="$listing  桌面条目：$DESKTOP_DST\n"
[[ -f "$ICON_DST_FILE" ]] && listing="$listing  图标：$ICON_DST_FILE\n"
[[ -d "$LOCALE_DST" ]]    && listing="$listing  译文：$LOCALE_DST/\n"

if [[ -z "$listing" ]]; then
    info "未发现已安装的文件，将直接执行安装。"
else
    if ! confirm "即将重新安装：\n\n$listing\n以上文件会被覆盖。\n\n配置与命令历史会保留：\n$CONFIG_DIR/\n\n是否继续？"; then
        exit 0
    fi
fi

# ---------------- 3. 删除旧文件（保留配置）----------------

rm -f "$BIN_DST" "$DESKTOP_DST" "$ICON_DST_FILE" 2>/dev/null
rm -rf "$LOCALE_DST"/*/LC_MESSAGES/run-dialog.mo 2>/dev/null

# 注意：**不删** $CONFIG_DIR —— 这正是「重装」的意义所在。
# 也不动 Super+R 快捷键：install.sh 会重新注册（幂等，已存在就跳过）。

# 清掉图标缓存，否则新图标可能不生效
if command -v gtk4-update-icon-cache >/dev/null 2>&1; then
    gtk4-update-icon-cache -q -t -f "$PREFIX/icons/hicolor" >/dev/null 2>&1 || true
elif command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f "$PREFIX/icons/hicolor" >/dev/null 2>&1 || true
fi

# ---------------- 4. 交给安装向导 ----------------

# install.sh 会做前置检查、装文件、问 PATH、提供启动选项。
# 已注册的快捷键不会被重复添加（其 own_binding_registered 判断是幂等的）。
exec "$SCRIPT_DIR/install.sh" "$@"
