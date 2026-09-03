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
}

pub(crate) type AppPackageResult<T> = Result<T, AppPackageError>;
