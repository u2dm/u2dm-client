use std::path::PathBuf;

use directories::ProjectDirs;

use crate::error::{AppError, Result};

pub struct AppConfig {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let dirs = ProjectDirs::from("", "", "UTDM")
            .ok_or_else(|| AppError::Config("Failed to determine data directory".into()))?;
        Ok(Self {
            data_dir: dirs.data_dir().to_path_buf(),
            cache_dir: dirs.cache_dir().to_path_buf(),
        })
    }
}
