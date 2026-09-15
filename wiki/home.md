# run-dialog wiki

欢迎。这里是 [run-dialog](https://github.com/Age10-Moyu/run-dialog) 的文档，
存放 README 装不下的内容 —— 主要是**改代码时才会用到**的资料。

README 面向使用者，Wiki 面向想动手的人。

---

## 从这里开始

| 页面 | 内容 |
|------|------|
| [开发指南](development.md) | 代码结构、各模块职责、往哪儿改、怎么调试 |
| [重新生成译文](translations.md) | 改过文案后如何更新 `po/*.po` 与 `.mo` |
| [打包](packaging.md) | 构建 deb、新增语言时的注意事项 |

---

## 快速索引

**想改界面文字** → 见「重新生成译文」，注意 msgid 是英文，别直接改中文

**想加一个设置项** → 见「开发指南 · 配置」

**想改命令查找规则** → [`src/launcher.rs`](../src/launcher.rs) 的 `Resolver::resolve`，
见「开发指南 · 查找顺序」

**想改 `.desktop` 解析** → [`src/desktop.rs`](../src/desktop.rs)，注意 Exec 字段码要符合 XDG 规范

**想改提权流程** → [`src/elevate.rs`](../src/elevate.rs)，涉及密码处理，改前先读该文件顶部注释

---

## 项目速览

```
src/
├── main.rs        界面、主窗口、命令分发、Config 读写
├── launcher.rs    7 步命令查找、终端判定
├── desktop.rs     .desktop 解析、Exec 字段码展开
├── elevate.rs     sudo askpass 提权、发布者与签名信息
├── i18n.rs        gettext 薄封装（t / tf / N_）
├── intro.rs       首次运行引导向导（7 页）
└── settings.rs    设置窗口

scripts/           构建、安装、打包、推送等辅助脚本
po/                gettext 译文
resources/         图标与 .desktop 模板
```

## 常用命令

```bash
./scripts/build.sh              # 编译（release）
./scripts/run.sh                # 运行
cargo test --release            # 139 项测试
./scripts/deb.sh                # 打 deb 包
./scripts/install.sh            # 安装到 ~/.local（图形向导）
./scripts/ostd.sh               # 一键：编译 + 安装
./scripts/push.sh "说明"         # 提交并推送（自动处理 WARP）
```

## 调试参数

仅 **debug 构建**可用（`--askpass` 除外，它是提权流程的一部分）：

```bash
./scripts/run.sh --resolve "文本编辑器" "ls -la" "https://example.com"
# 打印解析结果，不启动界面

./scripts/run.sh --resolve --pick "Text"
# 强制开启多候选选择

./scripts/run.sh --elevate-info
# 打印当前用户、是否管理员、提权时预填的用户名
```

> `run.sh` 只在参数含 `--resolve` 时保留 stdout，其余情况丢弃输出
> —— 对话框程序刷屏没意义。

## 子命令

| 命令 | 作用 |
|------|------|
| `run-dialog` | 打开运行对话框 |
| `run-dialog intro` | 首次运行引导（7 页向导） |
| `run-dialog settings` | 打开设置窗口 |

---

## 遇到问题

- **界面显示英文** → 见 [重新生成译文](translations.md) 的排查一节
- **编译报找不到 sccache / mold** → 删掉 `.cargo/` 或安装它们，见 README
- **改了文案但界面没变** → `.mo` 需要重新生成，见 [重新生成译文](translations.md)
