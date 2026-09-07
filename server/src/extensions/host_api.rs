//! Versioned Host API catalog and the capability-aware authorization facade.

use std::collections::BTreeMap;

use semver::{Version, VersionReq};

use crate::capabilities::CapabilityRegistry;

use super::error::{ExtensionError, ExtensionResult, PermissionError};
use super::manifest::ExtensionManifest;
use super::permissions::{Permission, PermissionSet};

pub(crate) const HOST_API_VERSION: &str = "1.0.0";

/// WIT and Rust use the same independently versioned host domains.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum HostApiDomain {
    Device,
    Vision,
    Input,
    Touch,
    Resource,
    Run,
    Runtime,
    Log,
    Media,
}

impl HostApiDomain {
    pub(crate) const ALL: [Self; 9] = [
        Self::Device,
        Self::Vision,
        Self::Input,
        Self::Touch,
        Self::Resource,
        Self::Run,
        Self::Runtime,
        Self::Log,
        Self::Media,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Vision => "vision",
            Self::Input => "input",
            Self::Touch => "touch",
            Self::Resource => "resource",
            Self::Run => "run",
            Self::Runtime => "runtime",
            Self::Log => "log",
            Self::Media => "media",
        }
    }

    pub(crate) fn all() -> &'static [Self; 9] {
        &Self::ALL
    }
}

impl std::fmt::Display for HostApiDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Host-side supported API versions. A domain can evolve independently while
/// preserving compatibility checks in the manifest boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostApiCatalog {
    versions: BTreeMap<HostApiDomain, Version>,
}

impl Default for HostApiCatalog {
    fn default() -> Self {
        let version = Version::parse(HOST_API_VERSION).expect("host API version is valid");
        Self {
            versions: HostApiDomain::ALL
                .into_iter()
                .map(|domain| (domain, version.clone()))
                .collect(),
        }
    }
}

impl HostApiCatalog {
    pub(crate) fn version(&self, domain: HostApiDomain) -> Option<&Version> {
        self.versions.get(&domain)
    }

    pub(crate) fn validate(&self, manifest: &ExtensionManifest) -> ExtensionResult<()> {
        for (domain, requirement) in manifest.host_api().iter() {
            let Some(supported) = self.version(*domain) else {
                return Err(ExtensionError::UnsupportedHostApi {
                    id: manifest.id().to_string(),
                    domain: domain.to_string(),
                    required: requirement.to_string(),
                    supported: "unavailable".to_string(),
                });
            };
            if !requirement.matches(supported) {
                return Err(ExtensionError::UnsupportedHostApi {
                    id: manifest.id().to_string(),
                    domain: domain.to_string(),
                    required: requirement.to_string(),
                    supported: supported.to_string(),
                });
            }
        }
        Ok(())
    }
}

/// A per-extension Host API view. The registry is only an adapter source;
/// callers still need an explicit permission for every operation.
#[derive(Clone)]
pub(crate) struct HostApi {
    registry: CapabilityRegistry,
    catalog: HostApiCatalog,
    permissions: PermissionSet,
}

impl HostApi {
    pub(crate) fn for_manifest(
        registry: CapabilityRegistry,
        catalog: HostApiCatalog,
        manifest: &ExtensionManifest,
    ) -> ExtensionResult<Self> {
        catalog.validate(manifest)?;
        Ok(Self {
            registry,
            catalog,
            permissions: manifest.permissions().clone(),
        })
    }

    pub(crate) fn authorize(&self, permission: Permission) -> ExtensionResult<()> {
        if self.permissions.allows(permission) {
            Ok(())
        } else {
            Err(ExtensionError::Permission(PermissionError::NotGranted(
                permission.as_str().to_string(),
            )))
        }
    }

    pub(crate) fn api_version(&self, domain: HostApiDomain) -> Option<&Version> {
        self.catalog.version(domain)
    }

    /// Reports whether an adapter for a domain is registered. Registration is
    /// separate from authorization so a missing backend cannot become an
    /// accidental permission escalation.
    pub(crate) fn domain_available(&self, domain: HostApiDomain) -> bool {
        match domain {
            HostApiDomain::Device => self.registry.device().is_some(),
            HostApiDomain::Vision => self.registry.vision().is_some(),
            HostApiDomain::Input => self.registry.input().is_some(),
            HostApiDomain::Touch => self.registry.touch().is_some(),
            HostApiDomain::Resource => self.registry.resource().is_some(),
            HostApiDomain::Run => self.registry.run().is_some(),
            HostApiDomain::Runtime => self.registry.runtime().is_some(),
            HostApiDomain::Log => self.registry.log().is_some(),
            // media 是 Core 进程级机制域（crate::media::service 单例恒可装配），
            // 不经 CapabilityRegistry 注册适配器。
            HostApiDomain::Media => true,
        }
    }

    pub(crate) fn registry(&self) -> &CapabilityRegistry {
        &self.registry
    }

    /// media 域入口（视频工作台 V1，实施合同 §1.2）：形态对齐 registry 域——
    /// 先按权限授权，再委托调用方提供的 Core 服务单例。见 [`MediaDomain`]。
    pub(crate) fn media(&self) -> MediaDomain<'_> {
        MediaDomain { api: self }
    }
}

/// media 域 facade（V1 薄封装）：方法只做权限校验，随后委托调用方传入的
/// Core 进程级服务单例（[`crate::media::MediaService`] /
/// [`crate::recording::RecordingService`]，经 `crate::media::service(&Config)`
/// 装配——宿主不持有 `Config`）。返回 `anyhow::Result` 以透传结构化
/// [`crate::media::MediaError`]（调用方按 kind 分派状态码），与
/// gamer_yaml 扩展消费 `HostApi::authorize` 的既有方式一致。
pub(crate) struct MediaDomain<'a> {
    api: &'a HostApi,
}

impl MediaDomain<'_> {
    fn require(&self, permission: Permission) -> anyhow::Result<()> {
        self.api.authorize(permission).map_err(anyhow::Error::new)
    }

    /// 导入素材字节（探测 + 原子落盘）。
    pub(crate) fn import(
        &self,
        media: &crate::media::MediaService,
        name: &str,
        bytes: &[u8],
    ) -> anyhow::Result<crate::media::MediaMetadata> {
        self.require(Permission::MediaImport)?;
        media.import_bytes(name, bytes)
    }

    pub(crate) fn get(
        &self,
        media: &crate::media::MediaService,
        id: &crate::media::MediaId,
    ) -> anyhow::Result<crate::media::MediaMetadata> {
        self.require(Permission::MediaRead)?;
        media.get(id)
    }

    pub(crate) fn list(
        &self,
        media: &crate::media::MediaService,
    ) -> anyhow::Result<Vec<crate::media::MediaMetadata>> {
        self.require(Permission::MediaRead)?;
        media.list()
    }

    /// 精确帧提取（PNG 字节）。
    pub(crate) fn open_frame(
        &self,
        media: &crate::media::MediaService,
        id: &crate::media::MediaId,
        request: &crate::media::FrameRequest,
    ) -> anyhow::Result<Vec<u8>> {
        self.require(Permission::MediaRead)?;
        media.extract_frame_png(id, request)
    }

    /// release = 删除素材（引用保护在 [`crate::media::MediaService::delete`] 内执行）。
    pub(crate) fn release(
        &self,
        media: &crate::media::MediaService,
        id: &crate::media::MediaId,
    ) -> anyhow::Result<()> {
        self.require(Permission::MediaWrite)?;
        media.delete(id)
    }

    /// 启动设备录制会话（设备独占）。
    pub(crate) fn record_start(
        &self,
        recording: &crate::recording::RecordingService,
        devices: &std::sync::Arc<crate::device::DeviceManager>,
        request: &crate::recording::RecordingStartReq,
    ) -> anyhow::Result<crate::recording::RecordingSessionMeta> {
        self.require(Permission::MediaRecord)?;
        recording.start(devices, request)
    }

    /// 停止并 finalize（幂等）。
    pub(crate) fn record_stop(
        &self,
        recording: &crate::recording::RecordingService,
        id: &crate::recording::RecordingId,
    ) -> anyhow::Result<crate::recording::RecordingSessionMeta> {
        self.require(Permission::MediaRecord)?;
        recording.stop(id)
    }

    /// 取消录制（已落盘部分保留为 interrupted 素材）。
    pub(crate) fn record_cancel(
        &self,
        recording: &crate::recording::RecordingService,
        id: &crate::recording::RecordingId,
    ) -> anyhow::Result<crate::recording::RecordingSessionMeta> {
        self.require(Permission::MediaRecord)?;
        recording.cancel(id)
    }

    /// 会话状态查询。
    pub(crate) fn record_status(
        &self,
        recording: &crate::recording::RecordingService,
        id: &crate::recording::RecordingId,
    ) -> anyhow::Result<crate::recording::RecordingSessionMeta> {
        self.require(Permission::MediaRecord)?;
        recording.status(id)
    }

    /// 读取会话操作事件（时间轴升序）。
    pub(crate) fn events(
        &self,
        recording: &crate::recording::RecordingService,
        id: &crate::recording::RecordingId,
    ) -> anyhow::Result<Vec<crate::recording::InputEventRecord>> {
        self.require(Permission::MediaEventsRead)?;
        recording.events(id)
    }
}

/// Keep the public requirement type behind the manifest API while still
/// allowing focused tests and future generated WIT adapters to inspect it.
pub(crate) type HostApiRequirement = VersionReq;
