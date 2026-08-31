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

    /// full 包内置离线组件包（UPDATE_CONTRACT §1：seeds/，运行期只读使用）。
    pub fn seeds_dir(&self) -> PathBuf {
        self.root.join("seeds")
    }

    /// 下载产物缓存中转区（cache/artifacts/，可随时清理重建）。
    pub fn artifacts_dir(&self) -> PathBuf {
        self.root.join("cache").join("artifacts")
    }

    /// 组件解压/校验临时区（与安装根同卷，staging→versions/runtime 才是原子 rename）。
    pub fn staging_dir(&self) -> PathBuf {
        self.root.join("staging")
    }

    /// 回滚失败或损坏组件的保留区（只增，不静默删除）。
    pub fn quarantine_dir(&self) -> PathBuf {
        self.root.join("quarantine")
    }

    /// 业务数据目录（用户数据，升级必须保留）。
    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    /// 用户配置文件（稳定路径契约 GB_CONFIG 注入值）。
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config").join("config.toml")
    }

    /// managed 组件目录 runtime/<id>/<version>/（哈希锚定的不可变组件目录）。
    pub fn component_dir(&self, id: &str, version: &str) -> PathBuf {
        self.runtime_dir().join(id).join(version)
    }
}
