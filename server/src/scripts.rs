//! 脚本文件存储：data/scripts/<package>/<name>.yaml（取代 SQLite scripts 表）
//!
//! package 语法：YAML 首个有效行写 `package <名字>`（非标准 YAML 指令，引擎解析前剥离），
//! 缺省 default。文件所在目录即 package；保存时按内容里的 package 归档，改名/改包即移动文件。
//!
//! 脚本包 zip 布局（导出/导入）：
//!   templates/<模板名>            （脚本依赖的模板图片，名字与 data/templates 下文件一致）
//!   script/<package>/<name>.yaml  （脚本自身 + call 递归依赖的子脚本）

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::PathBuf;

use serde::Serialize;

use crate::config::Config;

/// 未写 package 指令时的默认脚本包
pub const DEFAULT_PACKAGE: &str = "default";

/// 磁盘上的一个脚本文件（id = `package/name`，name 含 .yaml/.yml 扩展名）
#[derive(Debug, Clone, Serialize)]
pub struct ScriptFile {
    pub id: String,
    pub package: String,
    pub name: String,
    pub content: String,
    pub updated_at: String,
}

/// 解析脚本内容里的 package 指令：首个有效行 `package <名字>`
/// （空行与 # 注释行跳过；首个有效行不是指令则视为缺省，返回 None）
pub fn parse_package(content: &str) -> Option<String> {
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(rest) = t.strip_prefix("package") {
            if rest.starts_with(' ') || rest.starts_with('\t') {
                let pkg = rest.trim();
                if !pkg.is_empty() {
                    return Some(pkg.to_string());
                }
            }
        }
        return None;
    }
    None
}

/// 剥离 package 指令行（指令不是合法 YAML，serde_yaml 解析前必须去掉）
pub fn strip_package_line(content: &str) -> String {
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

/// 校验路径部件（package / 脚本文件名）：
/// 允许 unicode 字母数字与 `. - _`；禁止空、路径分隔符、`..`、前导点
fn sanitize_part(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() || t == "." || t == ".." || t.starts_with('.') {
        return None;
    }
    if t.chars().any(|c| !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_'))) {
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
    if t.chars().any(|c| !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '#' | ' '))) {
        return None;
    }
    Some(t.to_string())
}

pub struct ScriptStore {
    /// 脚本根目录（data/scripts），一级子目录 = package
    root: PathBuf,
    /// 模板目录（data/templates），导出时按依赖收集
    templates_dir: PathBuf,
}

impl ScriptStore {
    pub fn open(cfg: &Config) -> anyhow::Result<Self> {
        let root = cfg.data_dir.join("scripts");
        std::fs::create_dir_all(root.join(DEFAULT_PACKAGE))?;
        Ok(Self { root, templates_dir: cfg.data_dir.join("templates") })
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

    fn load_file(&self, package: &str, name: &str) -> Option<ScriptFile> {
        let p = self.root.join(package).join(name);
        if !p.is_file() {
            return None;
        }
        let content = std::fs::read_to_string(&p).ok()?;
        Some(ScriptFile {
            id: format!("{}/{}", package, name),
            package: package.to_string(),
            name: name.to_string(),
            content,
            updated_at: Self::fmt_mtime(&p),
        })
    }

    /// 列出全部脚本（按修改时间倒序，与旧 DB 版行为一致）
    pub fn list(&self) -> anyhow::Result<Vec<ScriptFile>> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return Ok(out);
        };
        for pkg_dir in rd.flatten() {
            if !pkg_dir.path().is_dir() {
                continue;
            }
            let package = pkg_dir.file_name().to_string_lossy().to_string();
            for f in std::fs::read_dir(pkg_dir.path())?.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                let low = name.to_lowercase();
                if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
                    continue;
                }
                if let Some(s) = self.load_file(&package, &name) {
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

    /// 保存脚本：package 取内容指令（缺省 default），name 缺扩展名时补 .yaml；
    /// old_id 存在且归档位置变化时移动（删旧文件）。返回落盘后的脚本。
    pub fn save(&self, old_id: Option<&str>, name: &str, content: &str) -> anyhow::Result<ScriptFile> {
        let pkg_raw = parse_package(content).unwrap_or_else(|| DEFAULT_PACKAGE.to_string());
        let package = sanitize_part(&pkg_raw)
            .ok_or_else(|| anyhow::anyhow!("package 名非法（只允许字母数字 . _ -）: {}", pkg_raw))?;
        let name_raw = name.trim();
        let mut name = sanitize_part(name_raw)
            .ok_or_else(|| anyhow::anyhow!("脚本名非法（只允许字母数字 . _ -）: {}", name_raw))?;
        let low = name.to_lowercase();
        if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
            name.push_str(".yaml");
        }
        let dir = self.root.join(&package);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(&name);
        std::fs::write(&path, content)?;
        let new_id = format!("{}/{}", package, name);
        if let Some(old) = old_id {
            if old != new_id {
                if let Some((opkg, oname)) = old.split_once('/') {
                    let old_path = self.root.join(opkg).join(oname);
                    if old_path != path && old_path.is_file() {
                        std::fs::remove_file(&old_path)?;
                        // 旧 package 目录已空则顺手删掉（default 保留）
                        if opkg != DEFAULT_PACKAGE {
                            let _ = std::fs::remove_dir(self.root.join(opkg));
                        }
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
        let path = self.root.join(pkg).join(name);
        std::fs::remove_file(&path).map_err(|e| anyhow::anyhow!("删除失败: {} ({})", e, path.display()))?;
        if pkg != DEFAULT_PACKAGE {
            let _ = std::fs::remove_dir(self.root.join(pkg)); // 目录非空时失败，忽略
        }
        Ok(())
    }

    /// call 子脚本按名解析：优先调用者同 package，其次全局；
    /// 名字缺 .yaml/.yml 扩展名时自动补全再试（call 写 `子脚本` 或 `子脚本.yml` 均可）
    pub fn resolve_call(&self, caller_pkg: &str, name: &str) -> anyhow::Result<Option<ScriptFile>> {
        let all = self.list()?;
        if let Some(i) = all.iter().position(|s| s.package == caller_pkg && s.name == name) {
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

    /// 导出脚本包：脚本 + 递归 call 依赖的子脚本 + 全部引用的模板 → zip 字节。
    /// 缺失的模板跳过并 warn（不阻断导出）。返回（建议文件名, zip 字节）。
    pub fn export(&self, id: &str) -> anyhow::Result<(String, Vec<u8>)> {
        let start = self.get(id)?.ok_or_else(|| anyhow::anyhow!("脚本不存在: {}", id))?;
        let mut scripts = vec![start.clone()];
        let mut templates: BTreeSet<String> = BTreeSet::new();
        let mut i = 0usize;
        while i < scripts.len() {
            let deps = scan_deps(&scripts[i].content);
            for c in &deps.calls {
                if let Some(sub) = self.resolve_call(&scripts[i].package, c)? {
                    if !scripts.iter().any(|s| s.id == sub.id) {
                        scripts.push(sub);
                    }
                }
            }
            for t in deps.templates {
                templates.insert(t);
            }
            i += 1;
        }
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for s in &scripts {
                zw.start_file(format!("script/{}/{}", s.package, s.name), opts)?;
                zw.write_all(s.content.as_bytes())?;
            }
            for t in &templates {
                let p = self.templates_dir.join(t);
                if !p.is_file() {
                    tracing::warn!(tpl = %t, "导出跳过缺失模板");
                    continue;
                }
                zw.start_file(format!("templates/{}", t), opts)?;
                zw.write_all(&std::fs::read(&p)?)?;
            }
            zw.finish()?;
        }
        let low = start.name.to_lowercase();
        let stem = if low.ends_with(".yaml") {
            &start.name[..start.name.len() - 5]
        } else if low.ends_with(".yml") {
            &start.name[..start.name.len() - 4]
        } else {
            start.name.as_str()
        };
        Ok((format!("{}.zip", stem), buf))
    }

    /// 导入脚本包。confirm=false 时只解析并报告同名冲突（前端二次确认），
    /// confirm=true 时落盘（同名替换）。包内 yaml 的 package 指令统一改写为所在目录。
    pub fn import(&self, bytes: &[u8], confirm: bool) -> anyhow::Result<ImportReport> {
        let mut rep = ImportReport::default();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        // 先全部解析到内存（zip-slip 防护 + 布局校验），无错才考虑落盘
        let mut files: Vec<(String, PathBuf, Option<String>, Vec<u8>)> = Vec::new();
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
            let (zip_path, dest, pkg): (String, PathBuf, Option<String>) = match comps.as_slice() {
                [t, name] if t == "templates" => {
                    let name = sanitize_template_name(name).ok_or_else(|| anyhow::anyhow!("模板名非法: {}", name))?;
                    (format!("templates/{}", name), self.templates_dir.join(name), None)
                }
                [s, pkg, name] if s == "script" => {
                    let pkg = sanitize_part(pkg).ok_or_else(|| anyhow::anyhow!("package 名非法: {}", pkg))?;
                    let name = sanitize_part(name).ok_or_else(|| anyhow::anyhow!("脚本名非法: {}", name))?;
                    let low = name.to_lowercase();
                    if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
                        anyhow::bail!("script/ 下只支持 .yaml/.yml 文件: {}", name);
                    }
                    (format!("script/{}/{}", pkg, name), self.root.join(&pkg).join(&name), Some(pkg))
                }
                _ => anyhow::bail!("包内路径需为 templates/<文件> 或 script/<package>/<脚本>: {}", f.name()),
            };
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            files.push((zip_path, dest, pkg, buf));
        }
        if files.is_empty() {
            anyhow::bail!("包内没有可导入的文件");
        }
        for (zip_path, dest, pkg, buf) in &mut files {
            if let Some(pkg) = &pkg {
                let content = String::from_utf8_lossy(buf).to_string();
                *buf = set_package_line(&content, pkg).into_bytes();
            }
            rep.entries.push(zip_path.clone());
            if dest.exists() {
                rep.conflicts.push(zip_path.clone());
            }
        }
        if !confirm {
            return Ok(rep);
        }
        for (zip_path, dest, _, buf) in files {
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

/// 依赖扫描结果
pub struct Deps {
    pub templates: Vec<String>,
    pub calls: Vec<String>,
}

/// 扫描脚本依赖：find/until 的模板名、click 为字符串时的模板名、call 的子脚本名
/// （递归遍历 steps / loop.steps / then / else 嵌套结构）
pub fn scan_deps(content: &str) -> Deps {
    let mut deps = Deps { templates: vec![], calls: vec![] };
    let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&strip_package_line(content)) else {
        return deps;
    };
    if let Some(steps) = doc.get("steps").and_then(|v| v.as_sequence()) {
        for s in steps {
            walk_deps(s, &mut deps);
        }
    }
    deps
}

fn walk_deps(v: &serde_yaml::Value, deps: &mut Deps) {
    if let Some(seq) = v.as_sequence() {
        for s in seq {
            walk_deps(s, deps);
        }
        return;
    }
    let Some(map) = v.as_mapping() else { return };
    for (k, val) in map {
        match k.as_str().unwrap_or("") {
            "find" | "until" | "click" => {
                if let Some(t) = val.as_str() {
                    deps.templates.push(t.to_string());
                }
            }
            "call" => {
                if let Some(t) = val.as_str() {
                    deps.calls.push(t.to_string());
                }
            }
            "loop" | "then" | "else" | "steps" => walk_deps(val, deps),
            _ => {}
        }
    }
}

/// 设置/替换内容中的 package 指令行（导入时把包内脚本归一到所在目录的 package，
/// 避免「文件在 foo/ 目录、内容却写 package bar」导致下次保存时被移走）
fn set_package_line(content: &str, pkg: &str) -> String {
    let directive = format!("package {}", pkg);
    if parse_package(content).is_some() {
        for line in content.lines() {
            let t = line.trim();
            let is_directive = !t.is_empty()
                && !t.starts_with('#')
                && t.strip_prefix("package")
                    .map(|r| r.starts_with(' ') || r.starts_with('\t'))
                    .unwrap_or(false);
            if is_directive {
                return content.replacen(line, &directive, 1);
            }
        }
    }
    format!("{}\n{}", directive, content)
}

/// 一次性迁移：SQLite scripts 表 → 文件系统（data/scripts/<package>/），
/// 并把 tasks.script_id 从旧 uuid 映射为 `<package>/<name>`。
/// 目录里已有脚本（迁移过/手动放过文件）时跳过。
pub fn migrate_from_db(db: &crate::store::Store, store: &ScriptStore) -> anyhow::Result<()> {
    if !store.list()?.is_empty() {
        return Ok(());
    }
    let legacy = db.legacy_scripts()?;
    if legacy.is_empty() {
        return Ok(());
    }
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (old_id, name, content) in legacy {
        match store.save(None, &name, &content) {
            Ok(s) => {
                tracing::info!(from = %old_id, to = %s.id, "脚本迁移");
                map.insert(old_id, s.id);
            }
            Err(e) => tracing::warn!(name = %name, "脚本迁移失败: {}", e),
        }
    }
    for mut t in db.list_tasks()? {
        if let Some(new_id) = map.get(&t.script_id) {
            let old = t.script_id.clone();
            t.script_id = new_id.clone();
            db.upsert_task(&t)?;
            tracing::info!(task = %t.name, from = %old, to = %new_id, "任务脚本引用已更新");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_directive() {
        assert_eq!(parse_package("package test\nsteps:\n  - log x\n").as_deref(), Some("test"));
        assert_eq!(parse_package("# 注释\n\npackage foo\nsteps: []").as_deref(), Some("foo"));
        assert_eq!(parse_package("steps:\n  - log x\n"), None);
        assert_eq!(parse_package(""), None);
        // package 后无名字 / 带冒号的 YAML 键 → 不算指令
        assert_eq!(parse_package("package\nsteps: []"), None);
        assert_eq!(parse_package("package: test\nsteps: []"), None);
        let stripped = strip_package_line("package test\nsteps:\n  - log x\n");
        assert_eq!(stripped, "steps:\n  - log x\n");
    }

    #[test]
    fn deps_scan() {
        let yaml = r#"
package demo
steps:
  - find: btn.png
    click: inner.png
    then:
      - until: done.png
    else:
      - call: sub.yml
  - loop:
      times: 2
      steps:
        - call: sub2
  - tap: [0.5, 0.5]
"#;
        let deps = scan_deps(yaml);
        assert_eq!(deps.templates, vec!["btn.png", "inner.png", "done.png"]);
        assert_eq!(deps.calls, vec!["sub.yml", "sub2"]);
    }

    #[test]
    fn set_package_directive() {
        assert_eq!(set_package_line("steps: []", "foo"), "package foo\nsteps: []");
        let replaced = set_package_line("package bar\nsteps: []", "foo");
        assert_eq!(replaced, "package foo\nsteps: []");
    }
}
