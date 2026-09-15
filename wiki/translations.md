# 重新生成译文

改过源码里的文案后，`.mo` 需要重新生成，否则界面不会变。

---

## 核心约定

- **msgid 一律英文**，源码里不出现中文
- 译文只存在于 `po/zh_CN.po`
- 域名为 `run-dialog`，`.mo` 文件名为 `run-dialog.mo`

---

## 完整流程

### 1. 把新增的文件加进 `po/POTFILES`

只列**含真实文案**的文件（`xgettext` 需要它作为输入清单）：

```
src/main.rs
src/settings.rs
src/intro.rs
```

> `i18n.rs` 不必列入 —— 其中出现的 `t()` / `tf()` 全是文档注释与单元测试。

### 2. 重新提取模板

```bash
xgettext --package-name=run-dialog --from-code=UTF-8 \
  --keyword=t --keyword=tf --keyword=N_ \
  --output=po/run-dialog.pot $(grep -v '^#' po/POTFILES | grep -v '^$')
```

**`--keyword=N_` 不能漏**（原因见下节）。

### 3. 合并到已有译文

```bash
msgmerge --update po/zh_CN.po po/run-dialog.pot
```

这一步会保留已翻译内容，新条目留空，改动过的条目标 `#, fuzzy`。

### 4. 翻译新条目

编辑 `po/zh_CN.po`，填写 `msgstr`。

**如果是 `#, fuzzy` 标记的条目，务必检查译文是否仍然贴切** ——
`msgmerge` 会基于相似度**猜测**译文，经常猜错：

| msgid | 被误配的译文 | 实际应为 |
|-------|------------|---------|
| `Runs programs as administrator, UAC style` | 以系统管理员权限创建此任务。 | 以管理员身份运行程序，仿 Windows UAC |
| `Locale name` | 用户名 | 区域名 |

确认无误后**删掉 `#, fuzzy` 行**，否则 `msgfmt` 会跳过该条目。

### 5. 校验

```bash
msgfmt --check-format --check-header -o /dev/null po/zh_CN.po
msgfmt --statistics -o /dev/null po/zh_CN.po
```

第二条会输出统计，理想结果是：

```
110 条已翻译消息.
```

**没有「未翻译」和「模糊」字样**。

### 6. 重新编译

```bash
./scripts/build.sh
```

`build.rs` 会自动把 `po/*.po` 编译到
`target/release/locale/<lang>/LC_MESSAGES/run-dialog.mo`。

**直接 `cargo build` 不会重新编译 `.mo`** —— 除非 `.po` 文件的时间戳变了。
若改了源码但没改 `.po`，`.mo` 不会重新生成（这是 `build.rs` 里
`cargo:rerun-if-changed=po` 的行为）。

---

## 关于 `N_()`

`xgettext` 只能识别字面量**直接**传给 `t()` 的写法：

```rust
t("Literal")                          // ✅ 能提取
let rows = [("Literal", "Title")];    // ❌ 提取不到
....title(&t(title))                  // 运行时才翻译
```

文案若先存进数组、之后才通过变量传给 `t()`，`xgettext` 就看不见了。
用 `N_()` 包住数组中的字符串即可：

```rust
use crate::i18n::{t, N_};

let rows = [
    (N_("Never goes through a shell"), N_("Arguments are passed ...")),
];

for (title, subtitle) in rows {
    ....title(&t(title))     // 运行时照常翻译
}
```

`N_()` 本身是**恒等函数**（`const fn N_(s: &'static str) -> &'static str`），
不产生翻译行为。**若误把它当 `t()` 使用，界面会显示英文原文。**

> 为什么是函数而不是宏？因为 `xgettext --keyword=N_` 识别的是**函数调用**
> 写法 `N_("...")`；宏需要写成 `N_!("...")`，提取不到。

---

## 排查：界面显示英文

依次检查：

### 1. 系统 locale 是否合法

```bash
echo $LANG
locale
```

**必须带编码后缀**：`zh_CN.UTF-8`，不能是 `zh_CN`。
缺少后缀时 GTK 会报 `Locale not supported by C library` 并回退到英文。

本机只装了 `zh_CN.utf8`（小写），`zh_CN.UTF-8` 也能识别。

### 2. `.mo` 是否安装到位

程序按以下顺序探测，**取第一个真正存在 `.mo` 文件的位置**：

1. `$RUN_DIALOG_LOCALEDIR`
2. `$XDG_DATA_HOME/locale`（默认 `~/.local/share/locale`）
3. 可执行文件同级 `locale/`
4. 可执行文件 `../share/locale`
5. `/usr/share/locale`

```bash
ls -la ~/.local/share/locale/zh_CN/LC_MESSAGES/run-dialog.mo
ls -la /usr/share/locale/zh_CN/LC_MESSAGES/run-dialog.mo
```

> **常见陷阱**：`~/.local/share/locale` 通常存在（别的程序装的），
> 但里面没有 `run-dialog.mo`。所以程序用的是 `has_catalog()` 检查
> **具体文件是否存在**，而不是检查目录存在与否。

### 3. 抓程序实际尝试打开的路径

```bash
strace -f -e trace=openat,newfstatat -o /tmp/t \
  env LANG=zh_CN.UTF-8 timeout 3 ./target/release/run-dialog
grep -o '"[^"]*run-dialog\.mo"' /tmp/t | sort -u
```

这是排查 locale 问题**最有效**的手段。

### 4. 单独验证某条译文

```bash
LC_ALL=zh_CN.UTF-8 TEXTDOMAINDIR=target/release/locale \
  gettext -d run-dialog -s "Welcome to Run"
```

期待输出「欢迎使用「运行」」。若输出英文原文，说明该条未翻译或 `.mo` 未更新。

---

## 新增语言

1. 建 `.po` 文件：`msginit --locale=xx --input=po/run-dialog.pot`
2. 加入 `po/LINGUAS`
3. **在 `Cargo.toml` 的 deb `assets` 里加一行** —— cargo-deb 的 assets
   **不支持通配符**，必须逐语言登记，否则 `.mo` 会被装到错误位置：

```toml
["target/release/locale/xx/LC_MESSAGES/run-dialog.mo",
 "usr/share/locale/xx/LC_MESSAGES/", "644"],
```

---

## 注意事项

- `xgettext` 不认识 `.rs` 扩展名，会按 C 语法解析，因此对源码里的字符字面量
  报「未结束的字符常量」警告 —— **可忽略**，不影响 `t()` / `tf()` 提取
- 本机 `en_US.UTF-8` **未安装**，测英文需用 `LC_ALL=C`
- `msgmerge` 会自动生成 `po/zh_CN.po~` 备份文件，已在 `.gitignore` 中排除
