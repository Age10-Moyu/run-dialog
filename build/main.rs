//! 构建脚本：把 `po/*.po` 编译成 `.mo`，安装到 `target/<profile>/locale/<lang>/LC_MESSAGES/`。
//!
//! 这样从源码直接运行时（`./run`）就能加载译文，无需先 `make install`。
//!
//! - 需要系统有 `msgfmt`（gettext 包）。缺失时给出警告并跳过，构建依然成功，
//!   只是运行时显示英文（msgid）。
//! - `.po` 变更会自动触发重新编译。

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=po");

    let out_dir = match std::env::var_os("OUT_DIR") {
        Some(d) => PathBuf::from(d),
        None => return,
    };

    // OUT_DIR 形如 target/<profile>/build/<pkg>-<hash>/out
    // 向上三级即 target/<profile>，把 locale/ 放在那里，与可执行文件同级。
    let profile_dir = match out_dir.ancestors().nth(3) {
        Some(d) => d.to_path_buf(),
        None => return,
    };
    let locale_root = profile_dir.join("locale");

    let po_dir = PathBuf::from("po");
    if !po_dir.is_dir() {
        return;
    }

    let entries = match std::fs::read_dir(&po_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut any = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("po") {
            continue;
        }
        let lang = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        println!("cargo:rerun-if-changed={}", path.display());

        let dest_dir = locale_root.join(&lang).join("LC_MESSAGES");
        if std::fs::create_dir_all(&dest_dir).is_err() {
            continue;
        }
        let dest = dest_dir.join("run-dialog.mo");

        // .po 比 .mo 新才重新编译
        if is_up_to_date(&path, &dest) {
            any = true;
            continue;
        }

        match msgfmt(&path, &dest) {
            Ok(()) => any = true,
            Err(err) => {
                println!("cargo:warning=msgfmt 编译 {} 失败：{}", path.display(), err);
            }
        }
    }

    if !any {
        println!("cargo:warning=未生成任何 .mo，程序将以英文（msgid）显示");
    }
}

fn is_up_to_date(src: &Path, dest: &Path) -> bool {
    let (Ok(s), Ok(d)) = (src.metadata(), dest.metadata()) else {
        return false;
    };
    let (Ok(st), Ok(dt)) = (s.modified(), d.modified()) else {
        return false;
    };
    dt >= st
}

fn msgfmt(src: &Path, dest: &Path) -> Result<(), String> {
    let output = Command::new("msgfmt")
        .arg("--check-format")
        .arg("--check-header")
        .arg("-o")
        .arg(dest)
        .arg(src)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "找不到 msgfmt，请安装 gettext".to_string()
            } else {
                e.to_string()
            }
        })?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}
