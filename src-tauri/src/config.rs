use rand::{seq::SliceRandom, Rng};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use uuid::Uuid;

const DEFAULT_PORT: u16 = 38987;
const DEFAULT_REFRESH_INTERVAL_SECONDS: u16 = 60;
const PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub device_id: Uuid,
    pub alias: String,
    pub listen_port: u16,
    pub save_dir: String,
    #[serde(default = "default_refresh_interval_seconds")]
    pub refresh_interval_seconds: u16,
    pub protocol_version: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateAppConfig {
    pub alias: String,
    pub listen_port: u16,
    pub save_dir: String,
    pub refresh_interval_seconds: u16,
}

pub fn load_or_create_config() -> Result<AppConfig, String> {
    let path = config_path()?;

    if path.exists() {
        let content = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let mut config: AppConfig = serde_json::from_str(&content).map_err(|error| error.to_string())?;
        if config.refresh_interval_seconds == 0 {
            config.refresh_interval_seconds = DEFAULT_REFRESH_INTERVAL_SECONDS;
            save_config(&config)?;
        }
        return Ok(config);
    }

    let config = AppConfig {
        device_id: Uuid::new_v4(),
        alias: generate_alias(),
        listen_port: DEFAULT_PORT,
        save_dir: default_save_dir()?.to_string_lossy().to_string(),
        refresh_interval_seconds: DEFAULT_REFRESH_INTERVAL_SECONDS,
        protocol_version: PROTOCOL_VERSION,
    };

    save_config(&config)?;
    Ok(config)
}

pub fn update_config(update: UpdateAppConfig) -> Result<AppConfig, String> {
    validate_alias(&update.alias)?;
    validate_port(update.listen_port)?;
    validate_refresh_interval(update.refresh_interval_seconds)?;

    let mut config = load_or_create_config()?;
    config.alias = update.alias.trim().to_string();
    config.listen_port = update.listen_port;
    config.save_dir = update.save_dir.trim().to_string();
    config.refresh_interval_seconds = update.refresh_interval_seconds;

    save_config(&config)?;
    Ok(config)
}

fn save_config(config: &AppConfig) -> Result<(), String> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let content = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    fs::write(path, content).map_err(|error| error.to_string())
}

fn config_path() -> Result<PathBuf, String> {
    Ok(app_config_dir()?.join("config.json"))
}

pub fn app_config_dir() -> Result<PathBuf, String> {
    dirs::config_dir()
        .map(|path| path.join("lansend"))
        .ok_or_else(|| "无法获取系统配置目录".to_string())
}

fn default_refresh_interval_seconds() -> u16 {
    DEFAULT_REFRESH_INTERVAL_SECONDS
}

fn default_save_dir() -> Result<PathBuf, String> {
    Ok(dirs::download_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| "无法获取默认保存目录".to_string())?
        .join("LanSend"))
}

fn generate_alias() -> String {
    let names = [
        "青山", "松月", "竹影", "星河", "云台", "书房", "海棠", "晨光", "林间", "北辰",
    ];
    let mut rng = rand::thread_rng();
    let prefix = names.choose(&mut rng).unwrap_or(&"Lan");
    let suffix: u16 = rng.gen_range(10..100);

    format!("{prefix}{suffix}")
}

fn validate_alias(alias: &str) -> Result<(), String> {
    let trimmed = alias.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 32 {
        return Err("别名长度必须为 1 到 32 个字符".to_string());
    }

    if !trimmed.chars().all(|character| {
        character.is_ascii_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(&character)
    }) {
        return Err("别名仅支持中文、英文和数字".to_string());
    }

    Ok(())
}

fn validate_port(port: u16) -> Result<(), String> {
    if !(1024..=49151).contains(&port) {
        return Err("端口必须在 1024 到 49151 之间".to_string());
    }

    Ok(())
}

fn validate_refresh_interval(seconds: u16) -> Result<(), String> {
    if !(5..=3600).contains(&seconds) {
        return Err("自动刷新间隔必须在 5 到 3600 秒之间".to_string());
    }

    Ok(())
}
