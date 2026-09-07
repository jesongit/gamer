//! Video Project（Phase 6）：`gamer.video` 的版本化制作项目数据模型与服务端
//! 保存期校验钩子。
//!
//! **存储形态**：项目 = Package 资源（`plugins/gamer.video/projects/<project-id>.json`），
//! 经既有资源 REST 读写（乐观并发 `expected_version` / dormant 保留语义）。
//! Core 只做三元组寻址，不解释 JSON 内容（ADR-11）；本模块是内容的唯一权威：
//!
//! - [`VideoProject`]：schema v1（`schema_version` 判别，非 1 统一拒绝、无迁移）。
//!   关联当前 Package（`package_id`）+ media assets **逻辑引用**（media_id +
//!   sha256/时长/帧数快照——**原视频不复制进 Package**）+ 可选 recording session
//!   引用 + 校准（[`ProjectCalibration`]，`version` 递增）+ 标记集合（帧身份 =
//!   `frame_index + pts_us + calibration_version`，绝不存浏览器浮点秒）+ 注释与
//!   制作进度。
//! - [`validate_project_content`]：保存期结构化校验（schema/版本/引用合法性）。
//! - [`VideoProjectResourceHandler`]：[`ResourceHandler`] 内容钩子，仅对
//!   `projects/*.json` 生效（其余路径透传 = 裸 Core 语义）。
//!
//! **注册缝（有意未接线）**：[`register_resource_handlers`] 供组合根（main.rs，
//! 集成者所有）引导期调用——与 gamer.yaml / gamer.keymap 同一机制；接线前
//! 服务端不强制校验，前端在保存前做同规则镜像校验（`web/src/components/video/
//! videoProject.js`）。媒体存在性诊断（素材缺失可打开）属运行时状态，由前端
//! 对媒体库列表重算，不进保存期校验。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 当前项目 schema 版本（唯一受支持版本；其他版本统一 `version.unsupported`）。
pub const PROJECT_SCHEMA_VERSION: u32 = 1;

/// 项目资源目录（相对 `plugins/gamer.video/`；目录语义归插件定义）。
pub const PROJECT_DIR: &str = "projects";

/// 单项目标记上限（防御性：项目是 1 MiB 文本资源，标记行均短文本）。
const MAX_MARKERS: usize = 500;
/// 单项目素材引用上限（V1 一主多参考）。
const MAX_ASSETS: usize = 16;

// ---------------------------------------------------------------------------
// schema v1
// ---------------------------------------------------------------------------

/// Video Project 文档（`projects/<id>.json` 的 wire 形态 = 存储形态）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoProject {
    pub schema_version: u32,
    /// 项目 id；必须等于所在资源文件名 stem（`projects/<id>.json`）。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// 所属 Package id（信息字段；与保存路径的包不一致时拒绝）。
    #[serde(default)]
    pub package_id: String,
    /// 项目级自由注释。
    #[serde(default)]
    pub notes: String,
    /// RFC3339。
    #[serde(default)]
    pub created_at: String,
    /// RFC3339。
    #[serde(default)]
    pub updated_at: String,
    /// 媒体素材**逻辑引用**（不复制原视频；缺失状态由前端按媒体库重算）。
    #[serde(default)]
    pub assets: Vec<ProjectAsset>,
    /// 自录会话引用（外部素材为 None；事件叠加据此决定是否可载入）。
    #[serde(default)]
    pub recording: Option<ProjectRecordingRef>,
    /// 校准（旋转/像素比例/有效画面区域/参考分辨率），`version` 递增版本化。
    pub calibration: ProjectCalibration,
    /// 标记集合（帧身份引用，非浮点秒）。
    #[serde(default)]
    pub markers: Vec<ProjectMarker>,
    /// 制作进度（自由阶段字符串 + 更新时间）。
    #[serde(default)]
    pub progress: Option<ProjectProgress>,
}

/// 项目内媒体素材引用快照（`role`: `primary` | `reference`；恰一个 primary）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectAsset {
    pub media_id: String,
    #[serde(default)]
    pub role: String,
    /// 导入/录制时的原始 sha256 快照（可空 = 未知；非空必须 64 hex）。
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub duration_us: Option<u64>,
    /// 快照（导入时帧表可能尚未解析，允许 None）。
    #[serde(default)]
    pub frame_count: Option<u64>,
}

/// 录制会话逻辑引用（事件时间轴的来源）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRecordingRef {
    pub recording_id: String,
}

/// 校准：encoded → oriented（旋转+像素比例）→ content（黑边裁剪）→ reference
/// （等比缩放，不静默非等比拉伸——变换本身只有单一缩放因子，见前端
/// `web/src/components/video/calibration.js`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectCalibration {
    /// 校准版本（≥1）；任何校准值变化必须递增。标记/模板区域/事件坐标记录
    /// 该版本，不一致 = 旧数据标脏提示重新确认，不悄悄变形。
    #[serde(default)]
    pub version: u32,
    /// 顺时针展示旋转角：0|90|180|270。
    #[serde(default)]
    pub rotation: u32,
    /// 像素宽高比（x/y），默认 1/1（方像素）；非 1 = 显示侧校正，不是拉伸。
    #[serde(default)]
    pub pixel_aspect: PixelAspect,
    /// oriented 域有效画面区域（黑边/录屏边框裁剪）；None = 全画面。
    #[serde(default)]
    pub content_rect: Option<ContentRect>,
    /// 制作参考分辨率（模板/脚本坐标空间）。
    pub reference_size: Size,
}

/// 像素宽高比（num/den ≥ 1 的整数比；1/1 = 方像素）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PixelAspect {
    pub num: u32,
    pub den: u32,
}

impl Default for PixelAspect {
    fn default() -> Self {
        Self { num: 1, den: 1 }
    }
}

/// 通用尺寸（像素，≥1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// oriented 域内容矩形（x/y ≥ 0，w/h ≥ 1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// 标记：引用确定帧身份（展示序索引 + PTS + 校准版本），注释可选。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMarker {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub note: String,
    pub frame: MarkerFrame,
    /// RFC3339。
    #[serde(default)]
    pub created_at: String,
}

/// 标记帧身份：`frame_index + pts_us`（服务端真实展示帧表）+ 记录时的校准版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkerFrame {
    pub media_id: String,
    pub frame_index: u64,
    pub pts_us: u64,
    pub calibration_version: u32,
}

/// 制作进度（阶段字符串由插件定义，≤32 字符；Core/他插件不解释）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectProgress {
    pub stage: String,
    #[serde(default)]
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// 结构化诊断（数组形态；api 层 400 `invalid_content` 原样透传）
// ---------------------------------------------------------------------------

/// 单条校验诊断：`code` 稳定机器码 / `message` 中文一句话 / `path` 定位。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
}

impl Diagnostic {
    fn new(code: &str, message: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            path: path.into(),
        }
    }
}

/// 解析项目 JSON：解析失败/校验失败统一返回诊断数组。
pub fn parse_project(content: &str) -> Result<VideoProject, Vec<Diagnostic>> {
    let project: VideoProject = serde_json::from_str(content).map_err(|error| {
        vec![Diagnostic::new(
            "json.parse",
            format!("项目 JSON 解析失败: {error}"),
            "",
        )]
    })?;
    let diagnostics = validate_project(&project);
    if diagnostics.is_empty() {
        Ok(project)
    } else {
        Err(diagnostics)
    }
}

/// 项目内容保存期校验（含文件路径一致性）。`Err` = 诊断数组 JSON。
pub fn validate_project_content(content: &str, package: &str, path: &str) -> Result<(), Value> {
    let project: VideoProject = serde_json::from_str(content).map_err(|error| {
        serde_json::to_value(vec![Diagnostic::new(
            "json.parse",
            format!("项目 JSON 解析失败: {error}"),
            "",
        )])
        .unwrap_or(Value::Null)
    })?;
    let mut diagnostics = validate_project(&project);
    if !project.package_id.is_empty() && project.package_id != package {
        diagnostics.push(Diagnostic::new(
            "package.mismatch",
            format!(
                "项目 package_id {:?} 与保存位置 Package {:?} 不一致",
                project.package_id, package
            ),
            "package_id",
        ));
    }
    let expected = format!("{PROJECT_DIR}/{}.json", project.id);
    if path.trim() != expected {
        diagnostics.push(Diagnostic::new(
            "id.path_mismatch",
            format!(
                "项目 id {:?} 必须与资源路径一致（{}）",
                project.id, expected
            ),
            "id",
        ));
    }
    if diagnostics.is_empty() {
        return Ok(());
    }
    Err(serde_json::to_value(diagnostics).unwrap_or(Value::Null))
}

/// 结构校验（不含保存位置上下文）。
pub fn validate_project(project: &VideoProject) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if project.schema_version != PROJECT_SCHEMA_VERSION {
        out.push(Diagnostic::new(
            "version.unsupported",
            format!(
                "项目 schema_version 仅支持 {}（得到 {}）",
                PROJECT_SCHEMA_VERSION, project.schema_version
            ),
            "schema_version",
        ));
    }
    validate_id(&project.id, &mut out);
    if project.name.trim().is_empty() {
        out.push(Diagnostic::new("name.required", "项目名不能为空", "name"));
    } else if project.name.chars().count() > 120 {
        out.push(Diagnostic::new(
            "name.too_long",
            "项目名超过 120 字符",
            "name",
        ));
    }
    validate_assets(project, &mut out);
    if let Some(recording) = &project.recording {
        if recording.recording_id.trim().is_empty() {
            out.push(Diagnostic::new(
                "recording.invalid",
                "录制会话引用 id 不能为空",
                "recording.recording_id",
            ));
        }
    }
    validate_calibration(project, &mut out);
    validate_markers(project, &mut out);
    if let Some(progress) = &project.progress {
        if progress.stage.chars().count() > 32 {
            out.push(Diagnostic::new(
                "progress.invalid",
                "制作进度 stage 超过 32 字符",
                "progress.stage",
            ));
        }
    }
    out
}

/// 项目 id 语法（与服务端 `validate_scope_id` 同规则：`[a-z0-9][a-z0-9._-]*`）。
fn validate_id(id: &str, out: &mut Vec<Diagnostic>) {
    let valid = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
        });
    if !valid {
        out.push(Diagnostic::new(
            "id.invalid",
            "项目 id 必须匹配 [a-z0-9][a-z0-9._-]*（≤64 字符，禁大写）",
            "id",
        ));
    }
}

fn validate_assets(project: &VideoProject, out: &mut Vec<Diagnostic>) {
    if project.assets.is_empty() {
        out.push(Diagnostic::new(
            "asset.required",
            "项目至少引用一个媒体素材",
            "assets",
        ));
        return;
    }
    if project.assets.len() > MAX_ASSETS {
        out.push(Diagnostic::new(
            "asset.too_many",
            format!("素材引用超过上限 {MAX_ASSETS}"),
            "assets",
        ));
    }
    let mut primaries = 0usize;
    for (index, asset) in project.assets.iter().enumerate() {
        let path = format!("assets[{index}]");
        if asset.media_id.trim().is_empty() {
            out.push(Diagnostic::new(
                "asset.media_id",
                "素材 media_id 不能为空",
                &path,
            ));
        }
        match asset.role.as_str() {
            "primary" => primaries += 1,
            "reference" => {}
            other => out.push(Diagnostic::new(
                "asset.role",
                format!("素材 role 只能是 primary|reference（得到 {other:?}）"),
                &path,
            )),
        }
        if !asset.sha256.is_empty()
            && (asset.sha256.len() != 64 || !asset.sha256.chars().all(|c| c.is_ascii_hexdigit()))
        {
            out.push(Diagnostic::new(
                "asset.sha256",
                "素材 sha256 快照必须是 64 位 hex 或留空",
                &path,
            ));
        }
    }
    if primaries != 1 {
        out.push(Diagnostic::new(
            "asset.primary_count",
            format!("项目必须恰有一个 primary 素材（得到 {primaries}）"),
            "assets",
        ));
    }
}

fn validate_calibration(project: &VideoProject, out: &mut Vec<Diagnostic>) {
    let calibration = &project.calibration;
    if calibration.version == 0 {
        out.push(Diagnostic::new(
            "calibration.version",
            "校准版本必须 ≥ 1（校准变化时递增）",
            "calibration.version",
        ));
    }
    if !matches!(calibration.rotation, 0 | 90 | 180 | 270) {
        out.push(Diagnostic::new(
            "calibration.rotation",
            format!("旋转只支持 0|90|180|270（得到 {}）", calibration.rotation),
            "calibration.rotation",
        ));
    }
    if calibration.pixel_aspect.num == 0 || calibration.pixel_aspect.den == 0 {
        out.push(Diagnostic::new(
            "calibration.pixel_aspect",
            "像素比例 num/den 必须 ≥ 1",
            "calibration.pixel_aspect",
        ));
    }
    if let Some(rect) = &calibration.content_rect {
        if rect.w == 0 || rect.h == 0 {
            out.push(Diagnostic::new(
                "calibration.content_rect",
                "有效画面区域宽高必须 ≥ 1",
                "calibration.content_rect",
            ));
        }
    }
    if calibration.reference_size.width == 0 || calibration.reference_size.height == 0 {
        out.push(Diagnostic::new(
            "calibration.reference_size",
            "参考分辨率宽高必须 ≥ 1",
            "calibration.reference_size",
        ));
    }
}

fn validate_markers(project: &VideoProject, out: &mut Vec<Diagnostic>) {
    if project.markers.len() > MAX_MARKERS {
        out.push(Diagnostic::new(
            "marker.too_many",
            format!("标记数超过上限 {MAX_MARKERS}"),
            "markers",
        ));
    }
    let asset_ids: std::collections::HashSet<&str> = project
        .assets
        .iter()
        .map(|asset| asset.media_id.as_str())
        .collect();
    let mut seen = std::collections::HashSet::new();
    for (index, marker) in project.markers.iter().enumerate() {
        let path = format!("markers[{index}]");
        if marker.id.trim().is_empty() {
            out.push(Diagnostic::new("marker.id", "标记 id 不能为空", &path));
        } else if !seen.insert(marker.id.as_str()) {
            out.push(Diagnostic::new(
                "marker.duplicate",
                format!("标记 id 重复: {:?}", marker.id),
                &path,
            ));
        }
        if marker.label.chars().count() > 120 {
            out.push(Diagnostic::new(
                "marker.label",
                "标记名超过 120 字符",
                &path,
            ));
        }
        if !asset_ids.contains(marker.frame.media_id.as_str()) {
            out.push(Diagnostic::new(
                "marker.media_not_found",
                format!(
                    "标记引用的素材 {:?} 不在项目 assets 内",
                    marker.frame.media_id
                ),
                &path,
            ));
        }
        if marker.frame.calibration_version == 0 {
            out.push(Diagnostic::new(
                "marker.calibration_version",
                "标记帧必须记录有效校准版本（≥1）",
                &path,
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// 资源内容钩子（与 gamer.yaml / gamer.keymap 同机制；注册缝见模块注释）
// ---------------------------------------------------------------------------

/// `projects/<id>.json` 判定（其余路径透传 = 裸 Core 语义）。
fn is_project_path(path: &str) -> bool {
    let path = path.trim();
    path.starts_with(PROJECT_DIR)
        && path.as_bytes().get(PROJECT_DIR.len()) == Some(&b'/')
        && path.ends_with(".json")
}

pub struct VideoProjectResourceHandler;

impl crate::resources::ResourceHandler for VideoProjectResourceHandler {
    fn validate_save(&self, req: crate::resources::SaveValidation<'_>) -> Result<(), Value> {
        if !is_project_path(req.path) {
            return Ok(());
        }
        validate_project_content(req.content, req.package, req.path)
    }

    /// 列表/读取注记：项目条目附 schema 摘要；解析失败标 `project_valid:false`
    /// 供前端显示诊断（读取本身不失败——dormant/损坏数据可诊断不可用）。
    fn annotate(&self, entries: &[(String, String)]) -> serde_json::Map<String, Value> {
        let mut out = serde_json::Map::new();
        for (path, content) in entries {
            if !is_project_path(path) {
                continue;
            }
            let summary = match parse_project(content) {
                Ok(project) => serde_json::json!({
                    "project_valid": true,
                    "project_name": project.name,
                    "asset_count": project.assets.len(),
                    "marker_count": project.markers.len(),
                    "project_schema_version": project.schema_version,
                }),
                Err(diagnostics) => serde_json::json!({
                    "project_valid": false,
                    "project_diagnostics": diagnostics,
                }),
            };
            out.insert(path.clone(), summary);
        }
        out
    }
}

/// 组合根注册缝：`extensions::video::project::register_resource_handlers(&packages)`。
/// 与 gamer.yaml / gamer.keymap 的同名单一职责一致——引导期注册，启动后不撤。
#[allow(
    dead_code,
    reason = "组合根接线缝：注册点在 main.rs（集成者所有），接线前校验由前端镜像规则承担"
)]
pub fn register_resource_handlers(store: &crate::resources::PackageStore) {
    store.register_handler(
        super::VIDEO_EXTENSION_ID,
        std::sync::Arc::new(VideoProjectResourceHandler),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{PackageStore, ResourceHandler};

    fn config_for(dir: &std::path::Path) -> crate::config::Config {
        crate::config::Config {
            data_dir: dir.to_path_buf(),
            ..crate::config::Config::default()
        }
    }

    fn sample_project(id: &str) -> VideoProject {
        VideoProject {
            schema_version: PROJECT_SCHEMA_VERSION,
            id: id.to_string(),
            name: "日常标记".to_string(),
            package_id: "hkrpg".to_string(),
            notes: String::new(),
            created_at: "2026-09-07T00:00:00Z".to_string(),
            updated_at: "2026-09-07T00:00:00Z".to_string(),
            assets: vec![ProjectAsset {
                media_id: "m1".to_string(),
                role: "primary".to_string(),
                sha256: "a".repeat(64),
                duration_us: Some(6_500_000),
                frame_count: Some(195),
            }],
            recording: Some(ProjectRecordingRef {
                recording_id: "rec-1".to_string(),
            }),
            calibration: ProjectCalibration {
                version: 1,
                rotation: 0,
                pixel_aspect: PixelAspect::default(),
                content_rect: None,
                reference_size: Size {
                    width: 1920,
                    height: 1080,
                },
            },
            markers: vec![ProjectMarker {
                id: "mk-1".to_string(),
                label: "开始点击".to_string(),
                note: String::new(),
                frame: MarkerFrame {
                    media_id: "m1".to_string(),
                    frame_index: 12,
                    pts_us: 333_000,
                    calibration_version: 1,
                },
                created_at: "2026-09-07T00:00:00Z".to_string(),
            }],
            progress: Some(ProjectProgress {
                stage: "markers".to_string(),
                updated_at: "2026-09-07T00:00:00Z".to_string(),
            }),
        }
    }

    fn to_json(project: &VideoProject) -> String {
        serde_json::to_string(project).unwrap()
    }

    #[test]
    fn valid_project_parses_and_round_trips() {
        let project = sample_project("daily-markers");
        let parsed = parse_project(&to_json(&project)).unwrap();
        assert_eq!(parsed, project);
        assert!(validate_project_content(
            &to_json(&project),
            "hkrpg",
            "projects/daily-markers.json"
        )
        .is_ok());
    }

    #[test]
    fn wrong_schema_version_is_rejected_without_fallback() {
        let mut project = sample_project("p1");
        project.schema_version = 2;
        let diagnostics = parse_project(&to_json(&project)).unwrap_err();
        assert!(diagnostics.iter().any(|d| d.code == "version.unsupported"));
        // 缺字段/未知字段：解析级拒绝（deny_unknown_fields）
        let broken = r#"{"schema_version":1,"id":"p1","name":"x","assets":[],"calibration":{"version":1,"reference_size":{"width":10,"height":10}},"scale_x":2}"#;
        let diagnostics = parse_project(broken).unwrap_err();
        assert!(
            diagnostics[0].code == "json.parse",
            "非 v1 结构统一 json.parse 诊断"
        );
    }

    #[test]
    fn id_and_package_must_match_save_location() {
        let project = sample_project("p1");
        // id 与路径不一致
        let err =
            validate_project_content(&to_json(&project), "any", "projects/other.json").unwrap_err();
        assert!(serde_json::to_string(&err)
            .unwrap()
            .contains("id.path_mismatch"));
        // package_id 与保存包不一致
        let err = validate_project_content(&to_json(&project), "other-pkg", "projects/p1.json")
            .unwrap_err();
        assert!(serde_json::to_string(&err)
            .unwrap()
            .contains("package.mismatch"));
    }

    #[test]
    fn asset_rules_enforce_exactly_one_primary() {
        let mut project = sample_project("p1");
        project.assets.clear();
        let diagnostics = validate_project(&project);
        assert!(diagnostics.iter().any(|d| d.code == "asset.required"));

        let mut project = sample_project("p1");
        project.assets[0].role = "reference".to_string();
        project.assets.push(ProjectAsset {
            media_id: "m2".to_string(),
            role: "reference".to_string(),
            sha256: String::new(),
            duration_us: None,
            frame_count: None,
        });
        let diagnostics = validate_project(&project);
        assert!(diagnostics.iter().any(|d| d.code == "asset.primary_count"));

        let mut project = sample_project("p1");
        project.assets[0].sha256 = "xyz".to_string();
        assert!(validate_project(&project)
            .iter()
            .any(|d| d.code == "asset.sha256"));
    }

    #[test]
    fn markers_must_reference_assets_and_calibration_version() {
        let mut project = sample_project("p1");
        project.markers[0].frame.media_id = "ghost".to_string();
        let diagnostics = validate_project(&project);
        assert!(diagnostics
            .iter()
            .any(|d| d.code == "marker.media_not_found"));

        let mut project = sample_project("p1");
        project.markers[0].frame.calibration_version = 0;
        assert!(validate_project(&project)
            .iter()
            .any(|d| d.code == "marker.calibration_version"));

        let mut project = sample_project("p1");
        project.markers.push(project.markers[0].clone());
        assert!(validate_project(&project)
            .iter()
            .any(|d| d.code == "marker.duplicate"));
    }

    #[test]
    fn calibration_values_are_range_checked() {
        let mut project = sample_project("p1");
        project.calibration.rotation = 45;
        project.calibration.pixel_aspect = PixelAspect { num: 0, den: 1 };
        project.calibration.reference_size = Size {
            width: 0,
            height: 10,
        };
        let diagnostics = validate_project(&project);
        for code in [
            "calibration.rotation",
            "calibration.pixel_aspect",
            "calibration.reference_size",
        ] {
            assert!(diagnostics.iter().any(|d| d.code == code), "缺 {code}");
        }
    }

    #[test]
    fn handler_validates_only_project_paths() {
        let handler = VideoProjectResourceHandler;
        let project = sample_project("p1");
        let content = to_json(&project);
        // projects/*.json：坏内容拒绝、好内容放行
        assert!(handler
            .validate_save(crate::resources::SaveValidation {
                package: "pkg",
                plugin: "gamer.video",
                path: "projects/p1.json",
                content: "not json",
                store: unreachable_store(),
            })
            .is_err());
        assert!(handler
            .validate_save(crate::resources::SaveValidation {
                // sample_project 的 package_id = "hkrpg"：一致时放行
                package: "hkrpg",
                plugin: "gamer.video",
                path: "projects/p1.json",
                content: &content,
                store: unreachable_store(),
            })
            .is_ok());
        // 非 projects 路径：透传（裸 Core 语义）
        assert!(handler
            .validate_save(crate::resources::SaveValidation {
                package: "pkg",
                plugin: "gamer.video",
                path: "other/thing.json",
                content: "anything",
                store: unreachable_store(),
            })
            .is_ok());
    }

    fn unreachable_store() -> &'static crate::resources::PackageStore {
        // SaveValidation.store 仅部分 handler 消费；项目校验不触达 store。
        // 用空 PackageStore 实例（临时目录）填充引用形状。
        unreachable_store_cell()
    }

    fn unreachable_store_cell() -> &'static crate::resources::PackageStore {
        use std::sync::OnceLock;
        static CELL: OnceLock<Box<crate::resources::PackageStore>> = OnceLock::new();
        let boxed = CELL.get_or_init(|| {
            let dir = std::env::temp_dir()
                .join(format!("gamer-video-project-test-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            Box::new(PackageStore::open(&config_for(&dir)).expect("store"))
        });
        boxed.as_ref()
    }

    #[test]
    fn annotate_summarizes_project_entries_and_flags_corruption() {
        let handler = VideoProjectResourceHandler;
        let good = to_json(&sample_project("p1"));
        let entries = vec![
            ("projects/p1.json".to_string(), good),
            ("projects/bad.json".to_string(), "{oops".to_string()),
            ("misc.txt".to_string(), "hello".to_string()),
        ];
        let annotated = handler.annotate(&entries);
        let good_summary = &annotated["projects/p1.json"];
        assert_eq!(good_summary["project_valid"], serde_json::json!(true));
        assert_eq!(good_summary["project_name"], serde_json::json!("日常标记"));
        assert_eq!(good_summary["marker_count"], serde_json::json!(1));
        let bad_summary = &annotated["projects/bad.json"];
        assert_eq!(bad_summary["project_valid"], serde_json::json!(false));
        assert!(annotated.get("misc.txt").is_none(), "非项目路径不注记");
    }

    #[test]
    fn registered_handler_is_invoked_through_store_validate_save() {
        let temp = tempfile::TempDir::new().unwrap();
        let store = PackageStore::open(&config_for(temp.path())).unwrap();
        register_resource_handlers(&store);
        let err = store
            .validate_save(crate::resources::SaveValidation {
                package: "pkg",
                plugin: "gamer.video",
                path: "projects/p1.json",
                content: "not json",
                store: &store,
            })
            .unwrap_err();
        assert!(err.is_array());
        // 同一 store 上 gamer.yaml 的资源不受 video 钩子影响（按 plugin-id 分域）
        assert!(store
            .validate_save(crate::resources::SaveValidation {
                package: "pkg",
                plugin: "gamer.yaml",
                path: "automations/a.yaml",
                content: "not json",
                store: &store,
            })
            .is_ok());
    }

    #[test]
    fn project_id_rule_matches_scope_id_syntax() {
        let mut project = sample_project("Bad-ID");
        assert!(validate_project(&project)
            .iter()
            .any(|d| d.code == "id.invalid"));
        project.id = "ok.id-1_x".to_string();
        assert!(validate_project(&project).is_empty());
    }
}
