#!/usr/bin/bash
#
# run-dialog 一键安装（One-Step-To-Done）
#
# 把「编译 → 安装 → 引导」串成一条命令，适合从仓库直接拉取后使用。
#
#     ./scripts/ostd.sh
#
# 实际工作分派给：
#   - 同目录的 ./build.sh        编译（release）
#   - 同目录的 ./install         图形化安装向导
#
# 若你只想重新安装（已编译过），可直接运行 ./scripts/install.sh。

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

APP_TITLE="运行 一键安装"

info() { zenity --info --title="$APP_TITLE" --width=420 --text="$1" 2>/dev/null; }
err()  { zenity --error --title="$APP_TITLE" --width=420 --text="$1" 2>/dev/null; }

# 没有 zenity 时退化为纯终端输出（ostd 可能被用在最小化环境里）
if command -v zenity >/dev/null 2>&1; then
    say() { info "$1"; }
else
    say() { printf '\n%s\n' "$1" >&2; }
fi

cd "$PROJECT_ROOT" || exit 1

# ---------------- 1. 检查工具链 ----------------

missing=()
command -v cargo >/dev/null 2>&1 || missing+=("cargo (Rust)")
command -v msgfmt >/dev/null 2>&1 || missing+=("msgfmt (gettext)")

if (( ${#missing[@]} > 0 )); then
    err "缺少必要工具：\n\n  ${missing[*]}\n\nDebian/Ubuntu 可执行：\n\n  sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev gettext cargo"
    exit 1
fi

# ---------------- 2. 编译 ----------------

printf '正在编译…\n' >&2
if ! "$SCRIPT_DIR/build.sh"; then
    err "编译失败。\n\n请手动运行 ./scripts/build.sh 查看完整错误信息。"
    exit 1
fi

BIN="$PROJECT_ROOT/target/release/run-dialog"
if [[ ! -x "$BIN" ]]; then
    err "编译结束后仍找不到可执行文件：\n$BIN"
    exit 1
fi

# ---------------- 3. 交给安装向导 ----------------

# install 自己会做前置检查、装文件、问 PATH、注册快捷键、选外观、说明语言，
# 所以这里不需要重复实现，直接转交即可。
exec "$SCRIPT_DIR/install.sh" "$@"
