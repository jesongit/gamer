//! Shared first-run configuration for standalone and full-package installation.
use crate::layout::InstallLayout;
use std::{fs, io, io::Write};

pub const CONFIG_TEMPLATE: &str = include_str!("../../release/config.default.toml");

/// Only create absent configuration; never replace user settings or credentials.
pub fn ensure_config(layout: &InstallLayout) -> io::Result<()> {
    fs::create_dir_all(layout.root.join("config"))?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(layout.config_file())
    {
        Ok(mut file) => {
            file.write_all(CONFIG_TEMPLATE.as_bytes())?;
            file.sync_all()
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_creates_complete_config_and_preserves_existing_user_config() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("gamer-bootstrap-{}-{nonce}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let layout = InstallLayout::resolve(Some(dir.clone()));
        ensure_config(&layout).unwrap();
        let config: toml::Value = fs::read_to_string(layout.config_file())
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(config["port"].as_integer(), Some(8443));
        assert!(config.get("data_dir").is_some());
        fs::write(
            layout.config_file(),
            "port = 18443\n# existing user settings\n",
        )
        .unwrap();
        ensure_config(&layout).unwrap();
        assert_eq!(
            fs::read_to_string(layout.config_file()).unwrap(),
            "port = 18443\n# existing user settings\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
