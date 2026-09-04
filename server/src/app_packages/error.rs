use std::io;

use thiserror::Error;

/// Errors returned by the App Package storage boundary.
#[derive(Debug, Error)]
pub(crate) enum AppPackageError {
    #[error("非法 AppPackageId: {0}")]
    InvalidAppPackageId(String),

    #[error("非法 Android package name: {0}")]
    InvalidAndroidPackage(String),

    #[error("非法安装版本: {0}")]
    InvalidInstalledVersion(String),

    #[error("非法资源路径: {0}")]
    InvalidResourcePath(String),

    #[error("manifest.toml 无效: {0}")]
    InvalidManifest(String),

    #[error("App Package 归档无效: {0}")]
    InvalidArchive(String),

    #[error("App Package 归档大小 {actual} 字节超过上限 {limit} 字节")]
    ArchiveTooLarge { actual: usize, limit: usize },

    #[error("App Package 已安装: {package}@{version}")]
    AlreadyInstalled { package: String, version: String },

    #[error("App Package 未安装: {package}@{version}")]
    NotInstalled { package: String, version: String },

    #[error("App Package 文件系统错误: {0}")]
    Io(#[from] io::Error),

    #[error("App Package ZIP 错误: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("App Package 卸载后的任务挂起通知失败: {0}")]
    TaskHook(String),

    #[error("归档 SHA-256 不匹配: 期望 {expected}，实际 {actual}")]
    Sha256Mismatch { expected: String, actual: String },

    #[error("Android package {android} 已由其他 App Package 激活: {active_package}@{active_version}；需先卸载或切换激活")]
    PrimaryConflict {
        android: String,
        active_package: String,
        active_version: String,
    },

    #[error("App Package 未激活: {0}")]
    NotActive(String),

    #[error("包内任务预设无效: {0}")]
    InvalidPreset(String),

    #[error("包内任务预设发布失败: {0}")]
    PresetHook(String),

    /// 本地编辑区（workspace）没有 package.toml：导出前必须先在工作区初始化元数据。
    #[error("工作区未初始化: {0}")]
    WorkspaceNotFound(String),

    /// 本地编辑区 `package.toml` 无效（与包内 manifest 同一套校验规则，仅文件语境不同）。
    #[error("package.toml 无效: {0}")]
    InvalidWorkspaceMetadata(String),

    /// 导出 preflight 失败：收集到的问题全量返回（首失败即停会逼用户多跑几轮）。
    #[error("导出 preflight 失败:\n{problems}")]
    PreflightFailed { problems: String },

    #[error("App Package 构建失败: {0}")]
    PackageBuildFailed(String),
}

impl AppPackageError {
    /// preflight 问题列表 → 单个错误（Display 内按行展开，供 400 消息直接展示）。
    pub(crate) fn preflight_failed(problems: Vec<String>) -> Self {
        Self::PreflightFailed {
            problems: problems.join("\n"),
        }
    }

    /// preflight 失败的机器码（api 层错误体 `code` 字段用）。
    pub(crate) fn is_preflight_failed(&self) -> bool {
        matches!(self, Self::PreflightFailed { .. })
    }
}

pub(crate) type AppPackageResult<T> = Result<T, AppPackageError>;
