#!/usr/bin/bash

set -e

# 从任意目录调用都能工作：切到项目根（本脚本的上一级）
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

# --resolve 是调试模式，需要看到 stdout，不要吞掉输出
if [[ " $* " == *" --resolve "* ]]; then
    ./target/release/run-dialog "$@"
else
    ./target/release/run-dialog "$@" &> /dev/null
fi
