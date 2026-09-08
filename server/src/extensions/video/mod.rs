//! gamer.video builtin 扩展（视频工作台 V1，实施合同 §5）。
//!
//! 纯进程内机制扩展：无 guest 字节、无常驻实例、无 Runner、无 start 参数。
//! 它的贡献只有两类：
//!
//! - **manifest**（[`VIDEO_EXTENSION_MANIFEST_TOML`]）：manifest v2 +
//!   `[execution] kind="builtin"`（执行体归宿主注册表
//!   `extensions/builtin.rs`）与 `runtime = "core"` 的 `VideoWorkbench`
//!   面板贡献（组件名的解释权在前端 core-component-registry）；
//!   打包源 `tools/plugins/gamer.video/manifest.toml` 与本常量逐字同步
//!   （下方测试锁）。
//! - **生命周期语义**：作为 builtin 扩展，`start` 只表示「进入 Running」
//!   （点亮 UI 贡献），由 [`crate::extensions::is_builtin_extension`]
//!   参与组合根的 `instance_free` 判定——不启动 WASM 实例、不注册
//!   Runner（ADR-13 钩子对本 id 幂等 no-op）。
//!
//! 能力边界（计划 §3.2）：本扩展**不**复制 YAML parser / 模板存储 / Runner；
//! 录制草稿生成由 gamer.yaml 的 call 动作（`automation.create_draft`）承担，
//! 本扩展只持有 manifest 与生命周期归属。媒体/录制能力由 Core 进程级服务
//! （`crate::media` / `crate::recording`）承载，插件不直接持有句柄。
//!
//! 制作业务（Phase 6）：Video Project（项目/标记/校准）归本扩展所有——
//! schema 与保存期校验在 [`project`]，数据存 Package 资源
//! `plugins/gamer.video/projects/<id>.json`（Core 只寻址不解释）。

pub(crate) mod project;

/// builtin 扩展 id（唯一归属本模块；无 guest、无 Runner、无 start 参数）。
pub(crate) const VIDEO_EXTENSION_ID: &str = "gamer.video";

/// Canonical manifest for the video workbench extension. The package still
/// has to be installed through the normal `.gplugin` service; keeping the
/// manifest here makes the extension's requested surface reviewable and gives
/// package builders one source of truth for the panel contribution.
///
/// 无 `[host_api]` 声明：本扩展是 builtin 机制扩展，不携带 guest、不经 WIT
/// 消费 Host API（media.* 权限目录供宿主域 facade 校验用，见
/// `host_api.rs` 的 `MediaDomain`）。
pub const VIDEO_EXTENSION_MANIFEST_TOML: &str = r#"manifest_version = 2
id = "gamer.video"
version = "1.0.0"
name = "视频工作台"
description = "视频工作台：媒体素材库、设备录制与操作草稿生成"
permissions = ["media.read", "media.import", "media.record", "media.write", "media.events.read"]

# 支持的 Android 应用（`*` = 通用；缺省/空同 `*`）。宿主不做硬门禁，
# Console 壳按当前设备应用过滤插件入口。
[targets.android]
packages = ["*"]

# builtin（宿主预置）执行类型：无 guest 字节、无常驻实例；执行体在宿主
# extensions/builtin.rs 注册表内（builtin_id 必须已注册才能安装）。
[execution]
kind = "builtin"
builtin_id = "gamer.video"

[[ui.contributions]]
# runtime = "core"：面板由宿主 Vue 组件渲染，component 键由前端
# core-component-registry 解释（VideoWorkbench = 视频工作台面板）。
panel_id = "video"
title = "视频"
icon = "🎬"
order = 28
location = "console.right"
runtime = "core"
requires_device = false
preferred_width = 440
component = "VideoWorkbench"
"#;

#[cfg(test)]
mod tests {
    use super::VIDEO_EXTENSION_MANIFEST_TOML;
    use crate::extensions::{
        parse_manifest, ExecutionKind, ExtensionService, ExtensionState, HostApiDomain, Permission,
    };

    /// 官方市场打包源（tools/plugins/gamer.video/manifest.toml）与本常量锁
    /// 同步：build-plugins.ps1 以文件为准打包，漂移会导致线上包与运行时语义
    /// 不一致。
    #[test]
    fn video_packaging_manifest_stays_in_sync_with_shipped_constant() {
        let packaged = include_str!("../../../../tools/plugins/gamer.video/manifest.toml");
        assert_eq!(
            VIDEO_EXTENSION_MANIFEST_TOML.trim(),
            packaged.trim(),
            "tools/plugins/gamer.video/manifest.toml 与 VIDEO_EXTENSION_MANIFEST_TOML 不一致"
        );
    }

    #[test]
    fn video_manifest_parses_with_builtin_execution_and_core_panel() {
        let manifest = parse_manifest(VIDEO_EXTENSION_MANIFEST_TOML.as_bytes()).unwrap();
        assert_eq!(manifest.id().as_str(), super::VIDEO_EXTENSION_ID);
        assert_eq!(manifest.version().as_str(), "1.0.0");
        // manifest v2 + builtin 执行类型：无 entry、builtin_id 已注册。
        assert_eq!(manifest.execution().kind(), ExecutionKind::Builtin);
        assert_eq!(
            manifest.execution().builtin_id(),
            Some(super::VIDEO_EXTENSION_ID)
        );
        assert!(manifest.entry().is_none());
        assert!(crate::extensions::builtin_extension(super::VIDEO_EXTENSION_ID).is_some());
        // media.* 权限闭集全量声明（A 侧目录，permissions.rs 为唯一权威）。
        for name in [
            Permission::MediaRead,
            Permission::MediaImport,
            Permission::MediaRecord,
            Permission::MediaWrite,
            Permission::MediaEventsRead,
        ] {
            assert!(
                manifest.permissions().allows(name),
                "manifest 必须声明 {}",
                name.as_str()
            );
        }
        // core 面板贡献：组件键归前端 core-component-registry 解释。
        let ui = manifest.ui();
        assert_eq!(ui.len(), 1);
        assert_eq!(ui[0].panel_id(), "video");
        assert_eq!(ui[0].component(), Some("VideoWorkbench"));
        assert!(matches!(
            ui[0].runtime(),
            crate::extensions::UiRuntime::Core
        ));
        assert!(ui[0].entry().is_none(), "core 贡献不能带 iframe entry");
        // 未声明 [host_api]：宿主域校验对空要求集恒通过。
        assert!(manifest.host_api().get(HostApiDomain::Media).is_none());
    }

    /// builtin 扩展生命周期：安装（无 plugin.wasm 的真实形态包）→ enable →
    /// start 进入 Running（不启动 guest 实例——无 WASM runtime 也必须成功）；
    /// UI 贡献仅 Running 可见：stop 即撤销，disable 保持撤销。
    #[tokio::test]
    async fn builtin_video_extension_starts_running_without_a_guest_instance() {
        use std::io::Write as _;
        let temp = tempfile::TempDir::new().unwrap();
        let service = ExtensionService::for_data_root(
            temp.path(),
            crate::capabilities::CapabilityRegistry::default(),
        );
        let mut archive = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut archive));
            let options = zip::write::SimpleFileOptions::default();
            writer
                .start_file(crate::extensions::MANIFEST_FILE_NAME, options)
                .unwrap();
            writer
                .write_all(VIDEO_EXTENSION_MANIFEST_TOML.as_bytes())
                .unwrap();
            // 真实 builtin 包：只有 manifest，没有 plugin.wasm。
            writer.finish().unwrap();
        }
        let installed = service.install(&archive).await.unwrap();
        assert_eq!(installed.id().as_str(), super::VIDEO_EXTENSION_ID);
        assert!(crate::extensions::is_builtin_extension(installed.id()));

        service.enable(installed.id()).await.unwrap();
        // 无 Runner：registrar 未挂载 / ADR-13 钩子对本 id no-op，start 纯粹
        // 表示进入 Running（builtin 分支不触达 runtime）。
        let running = service.start(installed.id()).await.unwrap();
        assert_eq!(running.state(), ExtensionState::Running);
        // 面板贡献随 Running 出现（D2 的 VideoWorkbench 挂载来源）。
        let ui = service.ui_contributions().unwrap();
        assert_eq!(ui.len(), 1);
        assert_eq!(ui[0].panel_id, "video");
        assert_eq!(ui[0].component.as_deref(), Some("VideoWorkbench"));

        let stopped = service.stop(installed.id()).await.unwrap();
        assert_eq!(stopped.state(), ExtensionState::Enabled);
        // Phase 1 语义收紧：UI 贡献仅 Running 可见——stop 即撤销。
        assert!(
            service.ui_contributions().unwrap().is_empty(),
            "stop 后 UI 贡献必须消失"
        );
        service.disable(installed.id()).await.unwrap();
        assert!(service.ui_contributions().unwrap().is_empty());
    }
}
