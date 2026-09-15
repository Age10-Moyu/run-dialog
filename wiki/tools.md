# 工具的使用

`scripts/` 目录下的脚本。它们不是构建系统的一部分，只是把常用操作包起来的便利入口
—— 每个都不长，出问题直接看源码就行。

---

## 总览

| 脚本 | 作用 | 需要什么 |
|------|------|---------|
| `build.sh` | 编译（release） | Rust + GTK4 开发包 |
| `run.sh` | 运行已编译的程序 | 已 `build.sh` |
| `clear.sh` | 清空构建产物（**破坏性**） | — |
| `deb.sh` | 打包为 `.deb` | `cargo-deb` |
| `install.sh` | 图形化安装向导（用户级） | zenity |
| `reinstall.sh` | 重新安装（保留配置） | zenity |
| `uninstall.sh` | 图形化卸载 | zenity |
| `ostd.sh` | 一键：编译 + 安装 | 上述全部 |
| `push.sh` | 提交并推送（自动处理 WARP） | git、可选 `warp-cli` |

所有脚本都可以**从任意目录调用** —— 内部会自己 `cd` 到项目根。

---

## build.sh / clear.sh / deb.sh

```bash
./scripts/build.sh     # cargo build --release
./scripts/clear.sh     # rm -r target/     ← 删除全部构建产物
./scripts/deb.sh       # cargo deb
```

`clear.sh` 是**破坏性**的：`target/` 里的所有东西都会没，下次构建要重来
（有 `sccache` 的话会快些）。

`deb.sh` 需要先装 `cargo-deb`：

```bash
cargo install cargo-deb
```

产物在 `target/debian/`，详见 [打包](packaging.md)。

---

## run.sh

```bash
./scripts/run.sh                    # 静默运行
./scripts/run.sh --resolve "vim"    # 调试模式，保留 stdout
```

**为什么有两种行为**：普通运行时输出被丢弃（`&> /dev/null`）—— 这是个对话框
程序，刷屏没意义。但 `--resolve` 的用途就是**打印解析结果**，必须保留 stdout。

判断逻辑很直接：

```bash
if [[ " $* " == *" --resolve "* ]]; then
    ./target/release/run-dialog "$@"          # 保留输出
else
    ./target/release/run-dialog "$@" &> /dev/null
fi
```

调试参数只在 **debug 构建**下生效（`--askpass` 除外），详见
[开发指南](development.md)。

---

## install.sh / ostd.sh / reinstall.sh / uninstall.sh

```bash
./scripts/install.sh      # 只安装（假设已编译）
./scripts/ostd.sh         # 一键：检查工具链 → 编译 → 调用 install.sh
./scripts/reinstall.sh    # 重装：删旧文件（保留配置）→ 调 install.sh
./scripts/uninstall.sh    # 卸载（可选保留配置）
```

**四者关系**：

```
ostd.sh
 ├─ 检查 cargo / msgfmt
 ├─ 调用 build.sh
 └─ exec install.sh   ← 装上后交给向导

reinstall.sh
 ├─ 确保已编译（否则先调 build.sh）
 ├─ 删旧文件（**不删配置**）
 └─ exec install.sh
```

`ostd.sh` 存在的意义是「一条命令从零到能用」。若已编译过只想重装，
用 `reinstall.sh` 比「先 uninstall 再 install」省事 —— **只问一次**，
而且不会丢掉偏好设置。

**安装位置**（全部在 `~/.local`，不需要 root）：

```
~/.local/bin/run-dialog
~/.local/share/applications/run-dialog.desktop
~/.local/share/icons/hicolor/256x256/apps/run-dialog.png
~/.local/share/locale/zh_CN/LC_MESSAGES/run-dialog.mo
~/.config/run-dialog/config.ini          （首次运行后生成）
```

`install.sh` 会依次：

1. 装文件（用 zenity 进度条）
2. 检查 `~/.local/bin` 是否在 `PATH` 中，不在则询问是否写入 `~/.profile`
3. 收尾询问是否立刻启动一次

> 偏好设置（快捷键、外观、语言）**不在这里设** —— 那些是
> `run-dialog intro` 的职责，见下节。

`uninstall.sh` 会先列出将要删除的文件，再单独询问是否保留配置与命令历史。

### reinstall.sh 的注意点

顺序是**先确认编译产物、再删旧文件** —— 否则编译失败时用户会落得
「旧的删了、新的没装上」的尴尬境地。

它**不删** `~/.config/run-dialog/`（这正是「重装」与「卸载」的区别），
也不动 Super+R 快捷键（`install.sh` 的注册是幂等的，已存在就跳过）。

---

## push.sh

```bash
./scripts/push.sh "Updated a, b"    # 提交并推送
./scripts/push.sh --no-commit       # 只推送现有提交
./scripts/push.sh                   # 交互式询问提交说明
```

流程：`git add -A` → `git commit` → 连 WARP → `git push` → 断 WARP。

**存在的原因**：推送 GitHub 需要代理（Cloudflare WARP），手动连了还得记得断。
脚本用 `trap ... EXIT INT TERM` 保证**无论成功、失败还是 Ctrl+C 都会断开**。

两个实现细节：

- `warp-cli connect` 是**异步**的，脚本会轮询状态直到真正 `Connected`
  再推送（最多 10 秒），否则前几次 push 会超时
- 兼容 `warp-cli` 与 `cloudflare-warp` 两个命令名（发行版之间不一致）

---

## 偏好设置在哪做

容易混淆的一点：

| 操作 | 谁负责 |
|------|--------|
| 装文件、PATH 检查 | `install.sh` |
| **快捷键（Super+R，含冲突检测）** | `run-dialog intro` |
| **外观（浅色/深色）** | `run-dialog intro` |
| **语言（locale 检查）** | `run-dialog intro` |
| Windows 兼容层、多候选选择 | `run-dialog intro` 或 `run-dialog settings` |

即：**装完用 `intro` 引导，之后用 `settings` 改**。

---

## 作者

DeepSeek。
