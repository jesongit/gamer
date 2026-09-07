//! Builtin（宿主预置）扩展实现注册表（计划 §5.2）。
//!
//! `execution.kind = "builtin"` 的包不携带 guest 字节：其执行体是编译进宿主
//! 的进程内实现。本注册表是**静态**的——只能由宿主代码在编译期登记，下载包
//! 无法扩展（`BUILTIN_EXTENSIONS` 是 const，无任何注册 API）。安装 builtin
//! 包时服务层校验 `builtin_id` 已注册，未注册 →
//! `ExtensionError::HostFeatureUnavailable`（结构化错误码
//! `host_feature_unavailable`，提示升级宿主而不是反复重装插件）。
//!
//! builtin 扩展允许无 guest、无常驻实例、无 Runner：`start` 只表示进入
//! Running（点亮 UI 贡献与 call 通路），见 `service.rs::instance_free`。

use super::model::ExtensionId;

/// 一个宿主预置扩展实现的描述。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BuiltinExtensionDescriptor {
    /// 注册 id；manifest 的 `[execution] builtin_id` 必须精确匹配。
    pub(crate) id: &'static str,
    /// 显示名（advisory，供管理界面展示）。
    pub(crate) name: &'static str,
    /// 描述（advisory）。
    pub(crate) description: &'static str,
    /// 该实现可用的最低宿主版本（advisory，如 ">=0.1.1"；服务端暂不做硬门禁）。
    pub(crate) host_version: &'static str,
}

/// 宿主预置扩展注册表（编译期固定；下载包不可扩展）。gamer.video 是首个注册项。
pub(crate) const BUILTIN_EXTENSIONS: &[BuiltinExtensionDescriptor] =
    &[BuiltinExtensionDescriptor {
        id: super::video::VIDEO_EXTENSION_ID,
        name: "视频工作台",
        description: "视频工作台：媒体素材库、设备录制与操作草稿生成",
        host_version: ">=0.1.1",
    }];

/// 按 id 查注册表。
pub(crate) fn builtin_extension(id: &str) -> Option<&'static BuiltinExtensionDescriptor> {
    BUILTIN_EXTENSIONS.iter().find(|entry| entry.id == id)
}

/// 该扩展是否为 builtin（宿主预置）执行类型：start 不启动任何 guest 实例。
/// 取代旧的 `video::is_native_extension` 字符串特判。
pub(crate) fn is_builtin_extension(id: &ExtensionId) -> bool {
    builtin_extension(id.as_str()).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    use crate::capabilities::CapabilityRegistry;
    use crate::extensions::{ExtensionError, ExtensionId, ExtensionService, ExtensionState};

    #[test]
    fn video_is_registered_and_unknown_ids_are_not() {
        let video = ExtensionId::parse(super::super::video::VIDEO_EXTENSION_ID).unwrap();
        assert!(is_builtin_extension(&video));
        let descriptor = builtin_extension("gamer.video").expect("gamer.video 必须已注册");
        assert_eq!(descriptor.name, "视频工作台");
        assert!(descriptor.host_version.starts_with(">="));

        let nobody = ExtensionId::parse("com.example.nobody").unwrap();
        assert!(!is_builtin_extension(&nobody));
        assert!(builtin_extension("com.example.nobody").is_none());
    }

    fn builtin_archive(id: &str, builtin_id: &str, with_wasm: bool) -> Vec<u8> {
        let manifest = format!(
            "manifest_version = 2\nid = \"{id}\"\nversion = \"1.0.0\"\nname = \"B\"\n\
             [execution]\nkind = \"builtin\"\nbuiltin_id = \"{builtin_id}\"\n"
        );
        let mut archive = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut archive));
            let options = zip::write::SimpleFileOptions::default();
            writer
                .start_file(crate::extensions::MANIFEST_FILE_NAME, options)
                .unwrap();
            writer.write_all(manifest.as_bytes()).unwrap();
            if with_wasm {
                // builtin 包伪装携带 plugin.wasm：安装必须拒绝（防执行类型伪装）
                writer.start_file("plugin.wasm", options).unwrap();
                writer.write_all(b"\0asm\x01\0\0\0").unwrap();
            }
            writer.finish().unwrap();
        }
        archive
    }

    #[tokio::test]
    async fn builtin_package_without_wasm_installs_and_starts_running() {
        let temp = tempfile::TempDir::new().unwrap();
        let service = ExtensionService::for_data_root(temp.path(), CapabilityRegistry::default());
        let archive = builtin_archive("gamer.video", "gamer.video", false);
        let installed = service.install(&archive).await.unwrap();
        assert_eq!(installed.id().as_str(), "gamer.video");
        assert!(is_builtin_extension(installed.id()));

        service.enable(installed.id()).await.unwrap();
        let running = service.start(installed.id()).await.unwrap();
        assert_eq!(running.state(), ExtensionState::Running);
        // 无 guest 字节也能列出（store 不对 builtin 做 entry 存在性检查）
        assert_eq!(service.list().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn builtin_package_carrying_plugin_wasm_is_rejected() {
        let temp = tempfile::TempDir::new().unwrap();
        let service = ExtensionService::for_data_root(temp.path(), CapabilityRegistry::default());
        let archive = builtin_archive("gamer.video", "gamer.video", true);
        let error = service.install(&archive).await.unwrap_err();
        assert!(
            matches!(error, ExtensionError::InvalidArchive(ref message) if message.contains("plugin.wasm")),
            "builtin 包携带 plugin.wasm 必须拒绝，得到 {error:?}"
        );
    }

    #[tokio::test]
    async fn unregistered_builtin_id_is_rejected_with_host_feature_unavailable() {
        let temp = tempfile::TempDir::new().unwrap();
        let service = ExtensionService::for_data_root(temp.path(), CapabilityRegistry::default());
        // 普通 wasm 包不得冒充未注册的 builtin id
        let archive = builtin_archive("com.example.fake", "com.example.nobody", false);
        let error = service.install(&archive).await.unwrap_err();
        assert!(
            matches!(error, ExtensionError::HostFeatureUnavailable(ref id) if id == "com.example.nobody"),
            "未注册 builtin_id 必须报 host_feature_unavailable，得到 {error:?}"
        );
        assert!(service.list().unwrap().is_empty());
    }
}
