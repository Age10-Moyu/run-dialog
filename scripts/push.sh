#!/usr/bin/bash
#
# 提交并推送（自动处理 Cloudflare WARP 连接）
#
# 用法：
#     ./scripts/push.sh                    # 用默认信息提交并推送
#     ./scripts/push.sh "提交说明"          # 指定提交说明
#     ./scripts/push.sh -m "说明"           # 同上
#     ./scripts/push.sh --no-commit        # 不提交，只推送现有提交
#
# 流程：
#     1. 若有未提交改动 → git add -A 并提交（无改动则跳过）
#     2. 连接 WARP → git push → 断开 WARP
#
# 无论推送成功、失败还是中途 Ctrl+C，都会尝试断开 WARP（见 trap）。

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT" || exit 1

# ---------------- 参数解析 ----------------

COMMIT_MSG=""
DO_COMMIT=1

while (( $# > 0 )); do
    case "$1" in
        -m)
            COMMIT_MSG="${2:-}"
            shift 2
            ;;
        --no-commit)
            DO_COMMIT=0
            shift
            ;;
        -*)
            echo "未知参数：$1" >&2
            exit 2
            ;;
        *)
            COMMIT_MSG="$1"
            shift
            ;;
    esac
done

# ---------------- WARP 管理 ----------------

# 选一个可用的命令名：发行版之间不一致
if command -v warp-cli >/dev/null 2>&1; then
    WARP="warp-cli"
elif command -v cloudflare-warp >/dev/null 2>&1; then
    WARP="cloudflare-warp"
else
    WARP=""
fi

warp_connected=0

# 只要还连着，就断开。注册到 EXIT，保证异常路径也会执行。
disconnect_warp() {
    (( warp_connected == 1 )) || return 0
    [[ -n "$WARP" ]] || return 0
    echo "===== 断开 Cloudflare WARP ====="
    "$WARP" disconnect >/dev/null 2>&1 || true
    warp_connected=0
}
trap disconnect_warp EXIT INT TERM

connect_warp() {
    if [[ -z "$WARP" ]]; then
        echo "警告：未找到 warp-cli，跳过 WARP 连接" >&2
        echo "      （若推送失败，请确认网络或代理设置）" >&2
        return 0
    fi

    echo "===== Cloudflare WARP 连接 ====="
    if ! "$WARP" connect >/dev/null 2>&1; then
        echo "警告：WARP 连接失败，继续尝试推送" >&2
        return 0
    fi
    warp_connected=1

    # warp-cli connect 是异步的，等它真正建立再推送，否则前几次 push 会超时
    local i
    for i in $(seq 1 20); do
        case "$("$WARP" status 2>/dev/null)" in
            *Connected*) return 0 ;;
        esac
        sleep 0.5
    done

    echo "警告：等待 WARP 就绪超时，继续尝试推送" >&2
    return 0
}

# ---------------- 1. 提交 ----------------

if (( DO_COMMIT == 1 )); then
    if [[ -n "$(git status --porcelain)" ]]; then
        if [[ -z "$COMMIT_MSG" ]]; then
            # 没给说明就交互询问；非交互环境下用时间戳兜底
            if [[ -t 0 ]]; then
                read -r -p "提交说明： " COMMIT_MSG
            fi
            [[ -n "$COMMIT_MSG" ]] || COMMIT_MSG="Update $(date +%Y-%m-%d\ %H:%M)"
        fi

        echo "===== 暂存改动 ====="
        git add -A || exit 1

        echo "===== 提交 ====="
        git commit -m "$COMMIT_MSG" || exit 1
    else
        echo "===== 无待提交改动，跳过提交 ====="
    fi
fi

# ---------------- 2. 推送 ----------------

connect_warp

echo "===== 推送 ====="
git push
status=$?

if (( status != 0 )); then
    echo >&2
    echo "推送失败（退出码 $status）。" >&2
fi

# 正常路径也显式断开，不等 trap（让输出顺序更直观）
disconnect_warp

exit $status
