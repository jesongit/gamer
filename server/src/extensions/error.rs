use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum PermissionError {
    #[error("未知插件权限: {0}")]
    Unknown(String),
    #[error("插件权限默认拒绝且不可授予: {0}")]
    Forbidden(String),
    #[error("插件未获授予权限: {0}")]
    NotGranted(String),
}

#[derive(Debug, Error)]
pub(crate) enum ExtensionError {
    #[error("插件 ID 无效: {0}")]
    InvalidId(String),
    #[error("插件版本无效: {0}")]
    InvalidVersion(String),
    #[error("插件归档路径无效: {0}")]
    InvalidPath(String),
    #[error("插件 manifest 无效: {0}")]
    InvalidManifest(String),
    #[error("插件归档无效: {0}")]
    InvalidArchive(String),
    #[error("插件归档大小 {actual} 字节超过上限 {limit} 字节")]
    ArchiveTooLarge { actual: usize, limit: usize },
    #[error("插件 {id}@{version} 已安装")]
    AlreadyInstalled { id: String, version: String },
    #[error("插件未安装: {id}")]
    NotInstalled { id: String },
    #[error("插件版本未安装: {id}@{version}")]
    VersionNotInstalled { id: String, version: String },
    #[error("插件 UI 未注册: {id}")]
    UiUnavailable { id: String },
    #[error("插件 {id} 的生命周期不允许执行 {operation}: 当前状态为 {state:?}")]
    InvalidTransition {
        id: String,
        operation: &'static str,
        state: super::model::ExtensionState,
    },
    #[error("插件 {id} 要求 Host API {domain} {required}，当前支持 {supported}")]
    UnsupportedHostApi {
        id: String,
        domain: String,
        required: String,
        supported: String,
    },
    #[error("WASM runtime 不可用: {0}")]
    RuntimeUnavailable(&'static str),
    #[error("WASM runtime 错误: {0}")]
    Runtime(String),
    #[error("插件权限错误: {0}")]
    Permission(#[from] PermissionError),
    #[error("插件状态文件无效: {0}")]
    InvalidState(String),
    #[error("插件文件系统错误: {0}")]
    Io(#[from] io::Error),
    #[error("插件 ZIP 错误: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("插件状态 JSON 错误: {0}")]
    Json(#[from] serde_json::Error),
}

pub(crate) type ExtensionResult<T> = Result<T, ExtensionError>;
