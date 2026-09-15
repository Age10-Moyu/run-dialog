# 开发指南

面向改代码的人。如果你只是想装来用，看 [README](../README.md) 就够了。

---

## 代码结构

```
src/
├── main.rs        1911 行   界面、主窗口、命令分发、Config 读写、命令历史
├── launcher.rs    1010 行   7 步命令查找、终端判定、Action
├── desktop.rs     1020 行   .desktop 解析、Exec 字段码展开、DesktopIndex
├── elevate.rs     1686 行   sudo askpass 提权、发布者与签名信息、ELF build-id
├── intro.rs        690 行   首次运行引导向导（7 页 Carousel）
├── i18n.rs         317 行   gettext 薄封装（t / tf / N_）
└── settings.rs     213 行   设置窗口
```

依赖关系是**单向**的：`main` 依赖所有模块，其余模块之间互不引用。

```
main ──┬── launcher ──── desktop
       ├── elevate
       ├── intro
       ├── settings
       └── i18n   ← 所有模块都用它
```

---

## 往哪儿改

| 想改什么 | 改哪里 | 注意 |
|---------|--------|------|
| 主窗口布局 | `main.rs` 的 `build_ui()` | — |
| 查找规则 | `launcher.rs` 的 `Resolver::resolve()` | 顺序有依赖，见下 |
| 终端判定 | `launcher.rs` 的 `needs_terminal_for_path()` | 名单在 `INTERACTIVE_INTERPRETERS` |
| `.desktop` 字段码 | `desktop.rs` 的 `expand_token()` | 必须符合 XDG 规范 |
| 提权流程 | `elevate.rs` | **涉及密码，先读文件顶部注释** |
| 引导向导 | `intro.rs` | 7 页，加页要改 `PAGE_COUNT` |
| 设置窗口 | `settings.rs` | 新开关要同步改 `Config` |
| 文案 | 源码里的 `t()` / `tf()` | 见 [重新生成译文](translations.md) |

---

## 查找顺序

`Resolver::resolve()` 按固定顺序逐级匹配，**顺序不能随意调整**：

1. **网址 / 已知协议** —— 仅白名单（`http` `https` `mailto` `ftp` `ssh` `file`…）
2. **显式路径** —— `/` `~` `./` `../` 开头，或含 `/`
3. **当前目录**
4. **系统目录** —— `/usr/local/bin` `/usr/bin` `/bin` `/usr/local/sbin` `/usr/sbin` `/sbin`
5. **`$PATH`**
6. **`.desktop` 索引** —— XDG、Flatpak、Snap 三处
7. **兜底 `xdg-open`**

> 第 1 步刻意用**白名单**而非 `a:b` 模糊匹配，否则 `C:\` 会被误判成协议。

**踩过的坑**：第 6 步必须用**完整输入**查询，不能用首 token ——
曾因此把 `Text Editor` 拆成 `Text`，误命中 gvim 的 `Keywords=Text;editor;`。

---

## 参数传递：一律 argv

**绝不拼 shell 命令**，`Action::Shell` 已删除。所有启动都是参数数组：

```rust
ls -la  →  Launch { program: "/usr/bin/ls", args: ["-la"] }
```

因此通配符、管道、变量**不会被解释**（这是刻意的）：

```
ls *.txt     →  参数为字面量 "*.txt"，不做 glob
ls | wc      →  参数为 "|" 和 "wc"
echo $HOME   →  参数为字面量 "$HOME"
```

唯一例外：`tty: true` 时需要在终端里拼一条命令给 `-e`，
所以 `run_in_terminal` / `shell_quote` 保留（仅此处用 shell 语义）。

---

## 配置

`Config` 定义在 `main.rs` 的 `config` 模块：

```rust
pub struct Config {
    pub enable_win_compat: bool,   // [experimental] win_compat
    pub pick_desktop: bool,        // [behavior] pick_desktop
}
```

**加一个新开关的步骤**：

1. `Config` 结构体加字段
2. `load()` 里读、`save()` 里写
3. `settings.rs` 加 `SwitchRow`（参考现有 `pick_row`）
4. 若用户点了「恢复默认」，`reset_btn` 的回调需同步该开关的界面状态
   —— 否则开关显示旧值，与刚写盘的配置不一致

配置文件位置：`~/.config/run-dialog/config.ini`

> 注意 `Config::load()` 每次都重新读盘，不缓存。这是为了让设置窗口与
> 引导向导同时打开时不会互相覆盖。

---

## 终端判定

三条依据，任一成立即 `tty = true`：

1. 对应 `.desktop` 标了 `Terminal=true`
2. 脚本的 shebang 指向交互式解释器（含 `#!/usr/bin/env X`）
3. 程序名在 `INTERACTIVE_INTERPRETERS` 名单里

第 3 条是必需的：`python` / `node` 本体是 **ELF**（不是脚本），
裸跑却要进 REPL。判定时按 basename 并**归一化版本后缀**：

```
python3.12 → python3
perl5.38   → perl5
```

**刻意不做**「ELF 未链接 GUI 库就包终端」—— 那会把 `ls` / `grep` 也包起来。
用户明确表示 `ls` 一闪而过可接受（同 Windows 的 `dir`）。

---

## 提权

改 `elevate.rs` 前**务必先读文件顶部注释**，那里记录了完整的密码流转设计。
几个关键约束：

- 用 `sudo -A` + `SUDO_ASKPASS`，**不要用 `-n`**（与 `-A` 冲突）
- fd 传递不可行：sudo 执行 askpass 时会重置 fd 表
- 不要定时删临时文件（不可靠且阻塞 GTK 主线程），用后台线程 `wait()`
- `--askpass` 模式必须在创建 GTK Application **之前** return

认证结果必须检查 —— 早先「忽略退出码 + 无条件返回 Ok」的实现，
在密码错误时会让 UAC 静默关闭、主窗口销毁，用户只看到界面消失。

---

## GTK 坑

**`set_text()` 会重置 `use_underline`** —— `_D` 变成字面下划线。
切换文本后必须重新 `set_use_underline(true)`：

```rust
label.set_text(&new_text);
label.set_use_underline(true);   // ← 少了这行助记符就失效
```

**子对话框关闭回调里绝不能 `app.quit()`** —— 会导致主窗口无法恢复。

**不要用 `set_visible(false)` 藏主窗口再恢复** —— 应让对话框
`transient_for(parent)` + `modal(true)` 盖在上面，这样输入内容保留、可原地重试。

---

## 测试

```bash
cargo test --release    # 139 项
```

按模块分布：`elevate` 46、`desktop` 43、`launcher` 34、`main` 9、`i18n` 8。

**写测试时注意**：

- 测试会读写**进程级环境变量**（`LANG` / `HOME`），相关用例已合并或加保存恢复，
  避免并行污染
- 临时文件必须用 `进程号 + 自增序号` 命名，固定名会并行冲突
- `HOME` 交换用 `static Mutex<()>` 串行化，参考 `main.rs` 的 `with_temp_home()`

---

## 提交

提交前：

```bash
cargo build --release     # 确认无警告
cargo test --release      # 确认测试通过
```

提交信息用 GitHub 默认风格（**动词 + 文件清单**）：

```
Updated README.md, intro.rs, main.rs
Created launcher.rs
Deleted zh_CN.po~
```

推送：

```bash
./scripts/push.sh "Updated a, b"
```

> 该脚本会自动连接 Cloudflare WARP、推送、再断开（GitHub 需要代理时用）。

---

## 作者

DeepSeek。
