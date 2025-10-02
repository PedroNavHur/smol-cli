use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Provider {
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Auth {
    pub api_key: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Runtime {
    pub temperature: f32,
    pub show_reasoning: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub provider: Provider,
    pub auth: Auth,
    pub runtime: Runtime,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            provider: Provider {
                base_url: "https://openrouter.ai/api/v1".into(),
                model: "grok-4-fast:free".into(),
            },
            auth: Auth { api_key: "".into() },
            runtime: Runtime {
                temperature: 0.2,
                show_reasoning: false,
            },
        }
    }
}

pub fn config_dir() -> Result<PathBuf> {
    let proj = ProjectDirs::from("ai", "smol", "smolcli")
        .ok_or_else(|| anyhow::anyhow!("could not determine config dir"))?;
    Ok(proj.config_dir().to_path_buf())
}

pub fn load() -> Result<AppConfig> {
    let mut cfg = AppConfig::default();

    // ENV overrides
    if let Ok(key) = std::env::var("OPENROUTER_API_KEY").or_else(|_| std::env::var("SMOL_API_KEY"))
    {
        cfg.auth.api_key = key;
    }
    if let Ok(model) = std::env::var("SMOL_MODEL") {
        cfg.provider.model = model;
    }
    if let Ok(url) = std::env::var("SMOL_BASE_URL") {
        cfg.provider.base_url = url;
    }

    // File config (if present)
    let dir = config_dir()?;
    let path = dir.join("config.toml");
    if path.exists() {
        let text = fs::read_to_string(&path).context("read config.toml")?;
        let file_cfg: AppConfig = toml::from_str(&text).context("parse config.toml")?;
        // file values only fill empty defaults/env
        if cfg.auth.api_key.is_empty() && !file_cfg.auth.api_key.is_empty() {
            cfg.auth.api_key = file_cfg.auth.api_key;
        }
        if std::env::var("SMOL_MODEL").is_err() {
            cfg.provider.model = file_cfg.provider.model;
        }
        if std::env::var("SMOL_BASE_URL").is_err() {
            cfg.provider.base_url = file_cfg.provider.base_url;
        }
        cfg.runtime = file_cfg.runtime;
    }

    Ok(cfg)
}

pub fn save(cfg: &AppConfig) -> Result<()> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir)?;
    let path = dir.join("config.toml");
    let s = toml::to_string_pretty(cfg)?;
    fs::write(path, s)?;
    Ok(())
}
