//! 脚本/模板文件存储：按应用分区 `data/<pkg>/yaml/` + `data/<pkg>/tmpl/`
//!
//! 分区 = 设备配置的应用包名（如 com.miHoYo.hkrpg），无 default 兜底；
//! 脚本 id = `<pkg>/<name>.yaml`（含 `/`，前端拼 URL 必须整体 encodeURIComponent）。
//! 旧 `package <名字>` YAML 指令已废除（引擎直接解析 YAML，残留指令行 = 解析报错），
//! 旧目录布局由 migrate_fs_layout 启动时一次性迁移并顺手剥离指令行。
//!
//! 分区快照 zip = 导出/导入同构（导出为整分区全量，不再按单个脚本收集依赖闭包）：
//!   yaml/<name>.yaml   分区内全部脚本
//!   tmpl/<模板名>       分区内全部模板图片
//! 导入必须显式指定目标分区（?pkg=）；两目录均可缺省。

use std::io::{Read, Write};
use std::path::PathBuf;

use serde::Serialize;

use crate::config::Config;

/// 磁盘上的一个脚本文件（id = `<pkg>/<name>`，name 含 .yaml/.yml 扩展名；package 字段 = 应用分区）
#[derive(Debug, Clone, Serialize)]
pub struct ScriptFile {
    pub id: String,
    pub package: String,
    pub name: String,
    pub content: String,
    pub updated_at: String,
}

/// 校验路径部件（应用包名 / 脚本文件名）：
/// 允许 unicode 字母数字与 `. - _`；禁止空、路径分隔符、`..`、前导点
pub fn sanitize_part(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() || t == "." || t == ".." || t.starts_with('.') {
        return None;
    }
    if t.chars()
        .any(|c| !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_')))
    {
        return None;
    }
    Some(t.to_string())
}

/// 校验模板文件名：允许 unicode 字母数字与 `. - _ #`（模板名可带 #x1_y1_x2_y2 区域后缀）、空格
fn sanitize_template_name(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() || t == "." || t == ".." || t.starts_with('.') {
        return None;
    }
    if t.chars()
        .any(|c| !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '#' | ' ')))
    {
        return None;
    }
    Some(t.to_string())
}

pub struct ScriptStore {
    /// 数据根目录（data/），一级子目录 = 应用分区（内含 yaml/ 与 tmpl/）
    root: PathBuf,
}

impl ScriptStore {
    pub fn open(cfg: &Config) -> anyhow::Result<Self> {
        Ok(Self {
            root: cfg.data_dir.clone(),
        })
    }

    /// 分区 yaml 脚本目录
    pub fn yaml_dir(&self, pkg: &str) -> PathBuf {
        self.root.join(pkg).join("yaml")
    }

    /// 分区模板目录
    pub fn tmpl_dir(&self, pkg: &str) -> PathBuf {
        self.root.join(pkg).join("tmpl")
    }

    /// 磁盘上全部分区名（存在 yaml/ 或 tmpl/ 子目录的一级目录，字典序）
    pub fn partitions(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for d in rd.flatten() {
            let p = d.path();
            if p.is_dir() && (p.join("yaml").is_dir() || p.join("tmpl").is_dir()) {
                out.push(d.file_name().to_string_lossy().to_string());
            }
        }
        out.sort();
        out
    }

    fn fmt_mtime(p: &std::path::Path) -> String {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_default()
    }

    fn load_file(&self, pkg: &str, name: &str) -> Option<ScriptFile> {
        let p = self.yaml_dir(pkg).join(name);
        if !p.is_file() {
            return None;
        }
        let content = std::fs::read_to_string(&p).ok()?;
        Some(ScriptFile {
            id: format!("{}/{}", pkg, name),
            package: pkg.to_string(),
            name: name.to_string(),
            content,
            updated_at: Self::fmt_mtime(&p),
        })
    }

    /// 列出全部脚本（按修改时间倒序，与旧 DB 版行为一致）
    pub fn list(&self) -> anyhow::Result<Vec<ScriptFile>> {
        let mut out = Vec::new();
        for pkg in self.partitions() {
            let Ok(rd) = std::fs::read_dir(self.yaml_dir(&pkg)) else {
                continue;
            };
            for f in rd.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                let low = name.to_lowercase();
                if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
                    continue;
                }
                if let Some(s) = self.load_file(&pkg, &name) {
                    out.push(s);
                }
            }
        }
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(out)
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Option<ScriptFile>> {
        let Some((pkg, name)) = id.split_once('/') else {
            return Ok(None);
        };
        Ok(self.load_file(pkg, name))
    }

    /// 保存脚本到指定应用分区，name 缺扩展名时补 .yaml；
    /// old_id 存在且归档位置变化时移动（删旧文件）。返回落盘后的脚本。
    pub fn save(
        &self,
        old_id: Option<&str>,
        pkg: &str,
        name: &str,
        content: &str,
    ) -> anyhow::Result<ScriptFile> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {}", pkg))?;
        let name_raw = name.trim();
        let mut name = sanitize_part(name_raw)
            .ok_or_else(|| anyhow::anyhow!("脚本名非法（只允许字母数字 . _ -）: {}", name_raw))?;
        let low = name.to_lowercase();
        if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
            name.push_str(".yaml");
        }
        let dir = self.yaml_dir(&package);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(&name);
        std::fs::write(&path, content)?;
        let new_id = format!("{}/{}", package, name);
        if let Some(old) = old_id {
            if old != new_id {
                if let Some((opkg, oname)) = old.split_once('/') {
                    let old_path = self.yaml_dir(opkg).join(oname);
                    if old_path != path && old_path.is_file() {
                        std::fs::remove_file(&old_path)?;
                        self.cleanup_partition(opkg);
                    }
                }
            }
        }
        Ok(ScriptFile {
            id: new_id,
            package,
            name,
            content: content.to_string(),
            updated_at: Self::fmt_mtime(&path),
        })
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        let Some((pkg, name)) = id.split_once('/') else {
            anyhow::bail!("非法脚本 id: {}", id);
        };
        let path = self.yaml_dir(pkg).join(name);
        std::fs::remove_file(&path)
            .map_err(|e| anyhow::anyhow!("删除失败: {} ({})", e, path.display()))?;
        self.cleanup_partition(pkg);
        Ok(())
    }

    /// 旧分区 yaml/tmpl 都已空时删掉分区目录（避免残留空目录被当成有效分区）
    pub fn cleanup_partition(&self, pkg: &str) {
        let _ = std::fs::remove_dir(self.yaml_dir(pkg)); // 非空时失败，忽略
        let _ = std::fs::remove_dir(self.tmpl_dir(pkg));
        let _ = std::fs::remove_dir(self.root.join(pkg));
    }

    /// call 子脚本按名解析：优先调用者同分区，其次跨分区；
    /// 名字缺 .yaml/.yml 扩展名时自动补全再试（call 写 `子脚本` 或 `子脚本.yml` 均可）
    pub fn resolve_call(&self, caller_pkg: &str, name: &str) -> anyhow::Result<Option<ScriptFile>> {
        let all = self.list()?;
        if let Some(i) = all
            .iter()
            .position(|s| s.package == caller_pkg && s.name == name)
        {
            let mut all = all;
            return Ok(Some(all.swap_remove(i)));
        }
        if let Some(i) = all.iter().position(|s| s.name == name) {
            let mut all = all;
            return Ok(Some(all.swap_remove(i)));
        }
        let low = name.to_lowercase();
        if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
            for ext in [".yaml", ".yml"] {
                let with_ext = format!("{}{}", name, ext);
                if let Some(i) = all.iter().position(|s| s.name == with_ext) {
                    let mut all = all;
                    return Ok(Some(all.swap_remove(i)));
                }
            }
        }
        Ok(None)
    }

    /// 导出整分区快照 zip：yaml/ 全部脚本 + tmpl/ 全部模板 → zip 字节。
    /// 分区没有任何可导出文件时报错。返回（建议文件名, zip 字节）。
    pub fn export_partition(&self, pkg: &str) -> anyhow::Result<(String, Vec<u8>)> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {}", pkg))?;
        // 收集规则与导入校验一致：yaml 只认 .yaml/.yml，tmpl 全部非隐藏文件
        let mut yaml_files: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.yaml_dir(&package)) {
            for f in rd.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                let low = name.to_lowercase();
                if f.path().is_file() && (low.ends_with(".yaml") || low.ends_with(".yml")) {
                    yaml_files.push(name);
                }
            }
        }
        let mut tmpl_files: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.tmpl_dir(&package)) {
            for f in rd.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                if f.path().is_file() && !name.starts_with('.') {
                    tmpl_files.push(name);
                }
            }
        }
        if yaml_files.is_empty() && tmpl_files.is_empty() {
            anyhow::bail!("分区 {} 没有可导出的脚本/模板", package);
        }
        yaml_files.sort();
        tmpl_files.sort();
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for name in &yaml_files {
                zw.start_file(format!("yaml/{}", name), opts)?;
                zw.write_all(&std::fs::read(self.yaml_dir(&package).join(name))?)?;
            }
            for name in &tmpl_files {
                zw.start_file(format!("tmpl/{}", name), opts)?;
                zw.write_all(&std::fs::read(self.tmpl_dir(&package).join(name))?)?;
            }
            zw.finish()?;
        }
        Ok((format!("{}.zip", package), buf))
    }

    /// 导入分区快照 zip 到指定应用分区。confirm=false 时只解析并报告同名冲突
    /// （前端二次确认），confirm=true 时落盘（同名替换）。只认 yaml/ 与 tmpl/ 布局。
    pub fn import(&self, bytes: &[u8], pkg: &str, confirm: bool) -> anyhow::Result<ImportReport> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {}", pkg))?;
        let mut rep = ImportReport::default();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        // 先全部解析到内存（zip-slip 防护 + 布局校验），无错才考虑落盘
        let mut files: Vec<(String, PathBuf, Vec<u8>)> = Vec::new();
        for i in 0..archive.len() {
            let mut f = archive.by_index(i)?;
            if f.is_dir() {
                continue;
            }
            // enclosed_name 拒绝绝对路径与 ..（zip-slip）
            let Some(rel) = f.enclosed_name() else {
                anyhow::bail!("包内路径非法: {}", f.name());
            };
            let comps: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect();
            let (zip_path, dest): (String, PathBuf) = match comps.as_slice() {
                [y, name] if y == "yaml" => {
                    let name = sanitize_part(name)
                        .ok_or_else(|| anyhow::anyhow!("脚本名非法: {}", name))?;
                    let low = name.to_lowercase();
                    if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
                        anyhow::bail!("yaml/ 下只支持 .yaml/.yml 文件: {}", name);
                    }
                    (
                        format!("yaml/{}", name),
                        self.yaml_dir(&package).join(&name),
                    )
                }
                [t, name] if t == "tmpl" => {
                    let name = sanitize_template_name(name)
                        .ok_or_else(|| anyhow::anyhow!("模板名非法: {}", name))?;
                    (
                        format!("tmpl/{}", name),
                        self.tmpl_dir(&package).join(&name),
                    )
                }
                _ => anyhow::bail!("包内路径需为 yaml/<脚本> 或 tmpl/<模板>: {}", f.name()),
            };
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            files.push((zip_path, dest, buf));
        }
        if files.is_empty() {
            anyhow::bail!("包内没有可导入的文件");
        }
        for (zip_path, dest, _) in &files {
            rep.entries.push(zip_path.clone());
            if dest.exists() {
                rep.conflicts.push(zip_path.clone());
            }
        }
        if !confirm {
            return Ok(rep);
        }
        for (zip_path, dest, buf) in files {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, &buf)?;
            if rep.conflicts.iter().any(|c| c == &zip_path) {
                rep.replaced.push(zip_path.clone());
            } else {
                rep.imported.push(zip_path.clone());
            }
        }
        Ok(rep)
    }
}

/// 导入结果报告
#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    /// 包内全部条目（zip 相对路径）
    pub entries: Vec<String>,
    /// 与现有文件同名、将被替换的条目（confirm=false 时的提示依据）
    pub conflicts: Vec<String>,
    /// 实际新增（confirm=true）
    pub imported: Vec<String>,
    /// 实际替换（confirm=true）
    pub replaced: Vec<String>,
}

/// 剥离旧 `package <名字>` 指令行（仅 migrate_fs_layout 清理旧文件残留用，
/// 运行时已不识别该指令——残留行会导致 YAML 解析报错）
fn strip_directive_line(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut stripped = false;
    for line in content.lines() {
        let t = line.trim();
        let is_directive = !stripped
            && !t.is_empty()
            && !t.starts_with('#')
            && t.strip_prefix("package")
                .map(|r| r.starts_with(' ') || r.starts_with('\t'))
                .unwrap_or(false);
        if is_directive {
            stripped = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 目录内（含子目录）是否有文件
fn dir_has_any_file(dir: &std::path::Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    for e in rd.flatten() {
        if e.path().is_file() {
            return true;
        }
        if e.path().is_dir() && dir_has_any_file(&e.path()) {
            return true;
        }
    }
    false
}

/// 一次性迁移：旧布局（data/scripts/<package>/ + data/templates/）→ 应用分区
/// （data/<目标pkg>/yaml|tmpl）。目标 = DB 首个配置了应用包名的设备；
/// 无设备 pkg 或目标分区已有数据时跳过（旧目录保留）。脚本内容顺手剥离旧指令行。
pub fn migrate_fs_layout(db: &crate::store::Store, store: &ScriptStore) -> anyhow::Result<()> {
    let old_scripts = store.root.join("scripts");
    let old_templates = store.root.join("templates");
    if !dir_has_any_file(&old_scripts) && !dir_has_any_file(&old_templates) {
        return Ok(());
    }
    let Some(pkg) = db
        .list_devices()?
        .into_iter()
        .filter_map(|d| d.pkg)
        .map(|p| p.trim().to_string())
        .find(|p| !p.is_empty())
    else {
        tracing::warn!("存在旧布局脚本/模板但无设备配置应用包名，跳过迁移（旧目录保留）");
        return Ok(());
    };
    let target_yaml = store.yaml_dir(&pkg);
    let target_tmpl = store.tmpl_dir(&pkg);
    if dir_has_any_file(&target_yaml) || dir_has_any_file(&target_tmpl) {
        tracing::warn!(pkg = %pkg, "目标分区已有数据，跳过旧布局迁移（旧目录保留）");
        return Ok(());
    }
    std::fs::create_dir_all(&target_yaml)?;
    std::fs::create_dir_all(&target_tmpl)?;
    if old_scripts.is_dir() {
        for pkg_dir in std::fs::read_dir(&old_scripts)?.flatten() {
            if !pkg_dir.path().is_dir() {
                continue;
            }
            for f in std::fs::read_dir(pkg_dir.path())?.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                let low = name.to_lowercase();
                if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
                    continue;
                }
                let dest = target_yaml.join(&name);
                if dest.exists() {
                    tracing::warn!(name = %name, "目标已存在同名脚本，跳过（旧文件保留）");
                    continue;
                }
                let content = std::fs::read_to_string(f.path())?;
                std::fs::write(&dest, strip_directive_line(&content))?;
                let _ = std::fs::remove_file(f.path());
                tracing::info!(from = %f.path().display(), to = %dest.display(), "脚本迁移至应用分区");
            }
            let _ = std::fs::remove_dir(pkg_dir.path()); // 非空（有跳过文件）时失败，忽略
        }
        let _ = std::fs::remove_dir(&old_scripts);
    }
    if old_templates.is_dir() {
        for f in std::fs::read_dir(&old_templates)?.flatten() {
            if !f.path().is_file() {
                continue;
            }
            let name = f.file_name().to_string_lossy().to_string();
            let dest = target_tmpl.join(&name);
            if dest.exists() {
                tracing::warn!(name = %name, "目标已存在同名模板，跳过（旧文件保留）");
                continue;
            }
            std::fs::rename(f.path(), &dest)?;
            tracing::info!(from = %f.path().display(), to = %dest.display(), "模板迁移至应用分区");
        }
        let _ = std::fs::remove_dir(&old_templates);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_legacy_directive() {
        assert_eq!(
            strip_directive_line("package test\nsteps:\n  - log x\n"),
            "steps:\n  - log x\n"
        );
        assert_eq!(
            strip_directive_line("# 注释\n\npackage foo\nsteps: []"),
            "# 注释\n\nsteps: []\n"
        );
        // 无指令行原样（补尾部换行）
        assert_eq!(strip_directive_line("steps: []"), "steps: []\n");
    }

    #[test]
    fn sanitize() {
        assert_eq!(
            sanitize_part("com.miHoYo.hkrpg").as_deref(),
            Some("com.miHoYo.hkrpg")
        );
        assert_eq!(sanitize_part(""), None);
        assert_eq!(sanitize_part(".."), None);
        assert_eq!(sanitize_part("a/b"), None);
        assert_eq!(sanitize_part("测试_1-2"), Some("测试_1-2".into()));
    }
}
