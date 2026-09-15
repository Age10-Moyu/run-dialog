#!/usr/bin/bash

set -e

# 从任意目录调用都能工作：切到项目根（本脚本的上一级）
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

cargo deb
