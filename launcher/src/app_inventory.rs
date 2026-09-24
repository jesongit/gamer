//! 从已校验 manifest 对应的完整 ZIP 建立应用逐文件清单，覆盖 EXE 和所有 Web 资源。
use crate::{layout::InstallLayout, repair::AppInstallSpec};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path};

#[derive(Serialize, Deserialize)]
struct Entry {
    path: String,
    size: u64,
    hash: String,
}
#[derive(Serialize, Deserialize)]
struct Inventory {
    artifact: String,
    files: Vec<Entry>,
}
fn path(layout: &InstallLayout, app: &AppInstallSpec) -> std::path::PathBuf {
    layout
        .state_dir()
        .join("app-inventory")
        .join(format!("{}.json", app.version))
}
/// 只在安全解压成功后调用；清单哈希从 ZIP 原文计算，不采信安装目录中的现存文件。
pub fn record(layout: &InstallLayout, app: &AppInstallSpec, zip_path: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(fs::File::open(zip_path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let mut hasher = Sha256::new();
        let mut buf = [0; 65536];
        loop {
            let n = entry.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        files.push(Entry {
            path: entry.name().to_string(),
            size: entry.size(),
            hash: crate::digest::to_hex(&hasher.finalize()),
        });
    }
    crate::state::atomic::write_json_atomic(
        &path(layout, app),
        &Inventory {
            artifact: app.artifact_sha256.clone(),
            files,
        },
    )
    .map_err(|e| e.to_string())
}
pub fn verify(
    layout: &InstallLayout,
    app: &AppInstallSpec,
    dir: &Path,
    deep: bool,
) -> Result<(), String> {
    let bytes = fs::read(path(layout, app)).map_err(|e| format!("应用文件清单不可用: {e}"))?;
    let inventory: Inventory = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if inventory.artifact != app.artifact_sha256 || inventory.files.is_empty() {
        return Err("应用文件清单与发行包不匹配".into());
    }
    for entry in inventory.files {
        if crate::manifest::pathsafe::check_single_path(&entry.path).is_some() {
            return Err("应用文件清单路径非法".into());
        }
        let result = if deep {
            crate::digest::verify_file(&dir.join(&entry.path), &entry.hash, entry.size)
        } else {
            crate::verification::verify(layout, &dir.join(&entry.path), &entry.hash, entry.size)
                .map(|_| ())
        };
        result.map_err(|e| format!("{}: {e}", entry.path))?;
    }
    Ok(())
}
