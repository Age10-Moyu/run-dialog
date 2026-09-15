# [¶](#run-dialog)run-dialog

在 Linux 上尽量还原 Windows 的“运行”窗口。  
Try to mimic Windows' "Run" window as closely as possible on Linux.

本仓库为国内互联网（尤其是勾槽的B站……）中一系列 Linux 无用项目<sup><a href="#fn-zh-1" id="ref-zh-1">[1]</a></sup>中的一员，也由这些项目启发。  
This repository belongs to a series of *useless Linux projects*<sup><a href="#fn-en-1" id="ref-en-1">[1]</a></sup> from the Chinese internet scene (Bilibili in particular...), and was inspired by them.

然而在一般情况下，本仓库（相较于 GNOME 原有运行窗口）被设计得更适合交互。  
That said, in ordinary use this dialog is designed to be more interactive than GNOME's built-in run dialog.

## [¶](#for-other-languages)For other languages

我很笨，只会中文、英文、粤语和客家话（？这俩列出来的意义是啥）  
I am not clever enough to speak anything but Chinese and English.

对于英文，本仓库已经尽力参照 Windows 中的 MUI 文件。  
For English, this project follows the wording used in Windows MUI files wherever possible.

然而，由于作者和 DeepSeek 都来自中国，肯定会有翻译不妥或未翻译您所在国家语言。若如此，您可自行 PR 仓库中的 PO 文件。  
However, since both the author and DeepSeek come from China, there are bound to be awkward translations, and your language may not be translated at all. If so, please send a pull request against the PO files in this repository.

技术无国界，但语言有。对不起 qwq  
Technology has no borders, but language does. Sorry qwq

---

## [¶](#简体中文版本)简体中文版本

此版本由我自己编写。~~但不知为何，AI 老是掺和进来。~~

### [¶](#适用平台)适用平台

任何使用 GNOME + Wayland 的 Linux 发行版。

逻辑上，适用于 KDE Plasma 与 X11，甚至 macOS。

显式不适用于 Windows。在 Windows 上运行本程序将会导致原生运行窗口被调起。

### [¶](#如何安装)如何安装

1. 直接拉取仓库并运行 `scripts/ostd.sh`（one-step-to-done），或
2. 添加 PPA 源 `ppa:age10-moyu/ppa`，但可能需要等我配置好 Launchpad。

无论通过何种方式安装，只需按照安装时运行的脚本指示即可。

`ostd.sh` 会依次完成：检查工具链 → 编译 → 调用图形化安装向导（装文件、注册快捷键、设置外观）。若程序已编译过、只想重新安装，可直接运行 `scripts/install.sh`。

#### [¶](#构建依赖)构建依赖

除 Rust 工具链与 GTK4/libadwaita 开发包外，本仓库还提供了 `.cargo/config.toml` 用于加速本机构建：

```toml
rustc-wrapper = "sccache"                      # 编译缓存
linker = "clang"                               # 链接器
rustflags = ["-C", "link-arg=-fuse-ld=mold"]   # 快速链接
```

**这三项不是必需的**。若你未安装，可任选一种处理：

- 安装它们（Debian/Ubuntu：`sudo apt install sccache clang mold`），或
- 直接删掉 `.cargo/` 目录，Cargo 会回退到默认工具链。

不处理而直接构建会报错，提示找不到 `sccache` 或 `mold`。

### [¶](#自编译)自编译

如果您对本仓库的默认配置不是很满意，或有更好的想法但不至于提交 PR，那么自编译将会是一个不错的选择。

根据 Wiki 页面完成程序的修改（您可能不需要改动原有程序）后，使用 `cargo build` 进行编译，并在编译完成后按照[下一章](#如何运行)进行运行。

如果需要，可以按照以下步骤构建 Debian 软件包：
``` bash
cargo install cargo-deb   # 安装 cargo-deb 工具（仅首次需要）
cargo deb                 # 构建，产物在 target/debian/
```
也可以直接用仓库里的脚本：`./scripts/deb.sh`。

### [¶](#如何运行)如何运行

0. 要运行 run-dialog，您必须配置了诸如 Wayland、Xorg、Xfce 等的窗口协议及 GNOME、KDE Plasma 等的桌面环境。
1. 运行 `scripts/build.sh` 后手动运行 `target/release/run-dialog`，或
2. ostd 或 apt 安装好（需要配置 PPA 源，方法参见[上一章节](#如何安装)）后直接在终端输入 `run-dialog`，或
3. [自编译](#自编译)完成后按照指示运行。

另附：在 Windows 环境下，运行程序将会导致原生运行窗口被拉起；未在 macOS 环境下测试过。

### [¶](#行为)行为

run-dialog 的行为视用户输入的内容而定。

- 如果用户输入一个 URI Scheme（例如 `ssh:hello@example.com`），将会尝试使用系统默认程序打开此 URI Scheme；
- 如果用户输入 `/path/to/file`，将会尝试使用系统默认程序打开此文件，若存在多个可使用的应用程序且系统未指定默认程序则会选择首个被发现 `.desktop` 文件的程序；
- 如果用户输入一个终端命令（例如 `python3 --version`），将会执行此命令，并为命中名单的程序打开终端窗口。

特殊地，输入 `control` 和 `ms-settings:` 将会打开 GNOME 设置面板；KDE 下不可用。

### [¶](#技术)技术

窗口使用 GTK 和 Zenity 来保证贴合 GNOME 原生样式。

后端使用 Rust 以迎合「全球 Rust 热」<sup><a href="#fn-zh-2" id="ref-zh-2">[2]</a></sup>。

前端文本参考了 Windows 下的「运行」窗口。

### [¶](#作者)作者

我（Age10_Moyu）与 DeepSeek V4.1 Flash。

### [¶](#协议)协议

GPLv3。本来我想用 [SCUL](https://github.com/Age10-Moyu/SCUL) 的，但 Launchpad 不准。

### [¶](#脚注)脚注

<a id="fn-zh-1"></a>\[1\] 具体来说，是将 Windows 下一些典范的「臃肿」项目移植到 Linux 下，以达到类似「忆苦思甜」的效果，同时好像也确实挺有趣的。<sup><a href="#ref-zh-1">↩</a></sup>

<a id="fn-zh-2"></a>\[2\] 例如，Ubuntu 26.04 LTS 的 `sudo` 就用 Rust 彻底重写了一遍；Cloudflare 一些核心框架使用的也是 Rust。冷知识：Rust 以高性能而闻名。<sup><a href="#ref-zh-2">↩</a></sup>

---

## [¶](#english-version)English version

This version was translated by DeepSeek.

### [¶](#platform)Platform

Any Linux distribution running GNOME + Wayland.

In principle it should also work on KDE Plasma, X11, or even macOS.

It explicitly does **not** work on Windows — running it there will simply bring up the native Run dialog.

### [¶](#installing)Installing

1. Clone the repository and run `scripts/ostd.sh` (one-step-to-done), or
2. Add the PPA `ppa:age10-moyu/ppa` — though you may have to wait until I get Launchpad set up.

Either way, just follow the instructions printed by the install script.

`ostd.sh` checks the toolchain, builds the project, then hands over to the graphical installer (which copies files, registers the shortcut and configures the appearance). If you have already built and only want to reinstall, run `scripts/install.sh` directly.

#### [¶](#build-dependencies)Build dependencies

Besides the Rust toolchain and the GTK4/libadwaita development packages, this repository ships a `.cargo/config.toml` that speeds up local builds:

```toml
rustc-wrapper = "sccache"                      # compilation cache
linker = "clang"                               # linker
rustflags = ["-C", "link-arg=-fuse-ld=mold"]   # fast linking
```

**None of these are mandatory.** If you do not have them installed, either:

- install them (Debian/Ubuntu: `sudo apt install sccache clang mold`), or
- delete the `.cargo/` directory — Cargo will fall back to the default toolchain.

Building without doing either will fail with an error about a missing `sccache` or `mold`.

### [¶](#building-from-source)Building from source

If you are not entirely happy with the defaults shipped by this repository, or you
have a better idea but not enough of one to send a pull request, building from
source is a good way to go.

Make your changes following the Wiki pages (you may not even need to touch the
existing code), then build with `cargo build` and run it as described in the
[next chapter](#running).

If you need one, a Debian package can be built like this:
``` bash
cargo install cargo-deb   # install the cargo-deb tool (first time only)
cargo deb                 # build; output goes to target/debian/
```
You can also just use the script in this repository: `./scripts/deb.sh`.

### [¶](#running)Running

0. To run run-dialog you need a windowing protocol such as Wayland, Xorg or Xfce, and a desktop environment such as GNOME or KDE Plasma.
1. Run `scripts/build.sh`, then launch `target/release/run-dialog` manually, or
2. After installing via ostd or apt (requires the PPA, see [Installing](#installing)), type `run-dialog` in a terminal.

One more note: on Windows this program will bring up the native Run dialog. It has not been tested on macOS.

### [¶](#behaviour)Behaviour

What run-dialog does depends on what you type.

- If you enter a URI scheme (for example `ssh:hello@example.com`), it opens that URI with the default handler.
- If you enter a path such as `/path/to/file`, it opens the file with the default application. When several applications are available and no default is configured, it picks the first `.desktop` file it finds.
- If you enter a terminal command (for example `python3 --version`), it executes it, launching a terminal window for programs on the interactive list.

As a special case, `control` and `ms-settings:` open the GNOME settings panel. This does not work on KDE.

### [¶](#technical-notes)Technical notes

The window uses GTK and Zenity so that it matches native GNOME styling.

The backend is written in Rust, to ride the "global Rust wave"<sup><a href="#fn-en-2" id="ref-en-2">[2]</a></sup>.

The UI text follows the Windows Run dialog.

### [¶](#author)Author

Me (Age10_Moyu) and DeepSeek V4.1 Flash.

### [¶](#licence)Licence

GPLv3. I originally wanted to use [SCUL](https://github.com/Age10-Moyu/SCUL), but Launchpad would not allow it.

### [¶](#footnotes)Footnotes

<a id="fn-en-1"></a>\[1\] Specifically, porting some of the finest "bloated" Windows projects to Linux — partly as a lesson in appreciating what we have, and partly because it is genuinely fun.<sup><a href="#ref-en-1">↩</a></sup>

<a id="fn-en-2"></a>\[2\] Ubuntu 26.04 LTS rewrote `sudo` entirely in Rust, and some of Cloudflare's core framework is written in Rust too. Fun fact: Rust is known for its performance.<sup><a href="#ref-en-2">↩</a></sup>

