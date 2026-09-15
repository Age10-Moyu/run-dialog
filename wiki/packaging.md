# 打包

如何构建 deb 包，以及改动打包配置时的注意事项。

---

## 快速构建

```bash
./scripts/deb.sh
```

产物在 `target/debian/run-dialog_<版本>-1_<架构>.deb`。

安装：

```bash
sudo dpkg -i target/debian/run-dialog_0.1.0-1_amd64.deb
```

> 也可用 `sudo apt install ./run-dialog_*.deb` —— apt 会自动补装依赖。
> 终端过小时 debconf 会打印「无法初始化前端界面：Dialog」并降级到 Readline，
> **这是无害的**（本包没有任何 debconf 交互脚本）。

---

## 配置位置

全部在 `Cargo.toml` 的 `[package.metadata.deb]` 段。

---

## 安装内容

```
/usr/bin/run-dialog
/usr/share/applications/run-dialog.desktop
/usr/share/icons/hicolor/256x256/apps/run-dialog.png
/usr/share/locale/zh_CN/LC_MESSAGES/run-dialog.mo
/usr/share/doc/run-dialog/copyright
/usr/share/doc/run-dialog/changelog.Debian.gz
```

对应 `assets` 配置：

```toml
assets = [
    ["target/release/run-dialog", "usr/bin/", "755"],
    ["resources/run-dialog.desktop", "usr/share/applications/", "644"],
    ["resources/icon.png", "usr/share/icons/hicolor/256x256/apps/run-dialog.png", "644"],
    ["target/release/locale/zh_CN/LC_MESSAGES/run-dialog.mo",
     "usr/share/locale/zh_CN/LC_MESSAGES/", "644"],
]
```

---

## 坑：assets 不支持通配符

cargo-deb 的 `assets` **不会展开 glob**。这样写是错的：

```toml
# ❌ 错误：会把文件扁平化装到 usr/share/locale/run-dialog.mo
["target/release/locale/*/LC_MESSAGES/run-dialog.mo", "usr/share/locale/", "644"]
```

必须**逐语言显式登记**：

```toml
# ✅ 正确
["target/release/locale/zh_CN/LC_MESSAGES/run-dialog.mo",
 "usr/share/locale/zh_CN/LC_MESSAGES/", "644"],
```

**新增语言时务必同步加一行**，详见 [重新生成译文](translations.md)。

---

## 坑：cargo-deb 的字段是固定集合

`[package.metadata.deb]` **没有 `authors`、也没有 `homepage`**。
写错会直接报 TOML 解析错误：

```
unknown field `authors`, expected one of `name`, `maintainer`, `copyright`, ...
```

各字段的正确来源：

| deb 字段 | 来源 |
|---------|------|
| `Maintainer` | deb 段的 `maintainer`（**必须**是 `名字 <邮箱>` 的 RFC 822 格式） |
| `Homepage` | 自动取自 `[package] homepage`，deb 段**不能**声明 |
| `Depends` | `depends = "$auto, xdg-utils"`，`$auto` 自动探测 GTK4/glib/adwaita |

> **踩过的坑**：若 deb 段缺失，cargo-deb 回退到 `[package] authors`。
> 裸名字（无邮箱）会触发 lintian 告警，dpkg 显示成 `Maintainer: Age10_Moyu`
> 而非完整格式。

---

## changelog

`changelog = "debian/changelog"` 会生成
`/usr/share/doc/run-dialog/changelog.Debian.gz`。

**缺失会报 lintian E 级错误** `no-changelog`。修改版本时要同步更新这个文件：

```
run-dialog (0.1.0-1) unstable; urgency=medium

  * Initial release.

 -- Age10_Moyu <yx_zyz20120418@163.com>  Tue, 15 Sep 2026 08:00:00 +0800
```

---

## 校验

用 lintian（Debian 官方打包检查器）：

```bash
lintian --tag-display-limit 0 target/debian/run-dialog_0.1.0-1_amd64.deb
```

当前只剩两条**无需处理**的提示：

| 标签 | 说明 |
|------|------|
| `initial-upload-closes-no-bugs` | 首次上传的正常现象，仅在向 Debian 官方仓库上传时才需处理 |
| `no-manual-page` | 无 man 手册，可选补 |

---

## 检查包内容

```bash
# 文件清单
dpkg-deb -c target/debian/run-dialog_0.1.0-1_amd64.deb

# 控制字段
dpkg-deb -f target/debian/run-dialog_0.1.0-1_amd64.deb

# 已安装包的信息
dpkg -l run-dialog
dpkg -L run-dialog
```

---

## 用户级安装 vs 系统级

两条路径并存，互不冲突：

| | 用户级 | 系统级 |
|---|--------|--------|
| 方式 | `./scripts/install.sh` | `sudo dpkg -i *.deb` |
| 位置 | `~/.local/` | `/usr/` |
| 需要 root | 否 | 是 |
| 更新方式 | 重新跑脚本 | `apt upgrade` |

`~/.local/bin` 通常在 `$PATH` 中**优先于** `/usr/bin`，
所以两者都装时实际运行的是用户级那份。

> **注意**：Super+R 快捷键如果指向裸命令 `run-dialog`，会命中 `~/.local/bin/`
> 那份。长期并存会导致「apt 更新了系统级，但实际跑的是用户级」的困惑。
> 建议二选一。
