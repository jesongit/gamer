//! 安装根目录布局（docs/UPDATE_CONTRACT.md §1）。

use std::path::{Path, PathBuf};

/// 安装根 = full ZIP 解压根；内部一切路径相对它解析，不依赖 cwd。
#[derive(Debug, Clone)]
pub struct InstallLayout {
    pub root: PathBuf,
}

impl InstallLayout {
    /// 显式 `--install-root` 优先；缺省取 exe 所在目录；均不可得时退回当前目录。
    pub fn resolve(explicit: Option<PathBuf>) -> Self {
        if let Some(root) = explicit {
            return Self { root };
        }
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        Self {
            root: exe_dir.unwrap_or_else(|| PathBuf::from(".")),
        }
    }

    pub fn state_dir(&self) -> PathBuf {
        self.root.join("state")
    }

    pub fn current_path(&self) -> PathBuf {
        self.state_dir().join("current.json")
    }

    pub fn manifests_dir(&self) -> PathBuf {
        self.root.join("manifests")
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join("runtime")
    }

    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
}
