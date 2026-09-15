#!/usr/bin/bash

set -e

# --resolve 是调试模式，需要看到 stdout，不要吞掉输出
if [[ " $* " == *" --resolve "* ]]; then
    ./target/release/run-dialog "$@"
else
    ./target/release/run-dialog "$@" &> /dev/null
fi
