use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::Manager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZabbixConfig {
    pub name: String,
    pub url: String,
    pub api_token: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    #[serde(default)]
    pub host_groups: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrafanaConfig {
    pub name: String,
    pub url: String,
    pub api_token: Option<String>,
    pub org_id: Option<u64>,
    #[serde(default)]
    pub folders: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UptimeKumaConfig {
    pub name: String,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub api_token: Option<String>,
    #[serde(default)]
    pub monitor_groups: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_refresh_interval")]
    pub refresh_interval_seconds: u64,
    #[serde(default = "default_max_resolved_history")]
    pub max_resolved_history: usize,
    #[serde(default)]
    pub zabbix: Vec<ZabbixConfig>,
    #[serde(default)]
    pub grafana: Vec<GrafanaConfig>,
    #[serde(default)]
    pub uptime_kuma: Vec<UptimeKumaConfig>,
}

fn default_refresh_interval() -> u64 {
    15
}

fn default_max_resolved_history() -> usize {
    100
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            refresh_interval_seconds: 15,
            max_resolved_history: 100,
            zabbix: vec![
                ZabbixConfig {
                    name: "Zabbix Server 1".to_string(),
                    url: "https://zabbix1.example.com".to_string(),
                    api_token: Some("your_zabbix_api_token_here".to_string()),
                    username: None,
                    password: None,
                    host_groups: vec!["Linux Servers".to_string(), "Database Servers".to_string()],
                },
                ZabbixConfig {
                    name: "Zabbix Server 2 (All Groups)".to_string(),
                    url: "https://zabbix2.example.com".to_string(),
                    api_token: None,
                    username: Some("Admin".to_string()),
                    password: Some("zabbix".to_string()),
                    host_groups: vec![], // Empty list = Query all host groups
                },
            ],
            grafana: vec![
                GrafanaConfig {
                    name: "Grafana Prod".to_string(),
                    url: "https://grafana.example.com".to_string(),
                    api_token: Some("glsa_your_api_token_here".to_string()),
                    org_id: Some(1),
                    folders: vec!["Infrastructure".to_string(), "Critical Alerts".to_string()],
                },
                GrafanaConfig {
                    name: "Grafana Stg (All Folders)".to_string(),
                    url: "https://grafana-stg.example.com".to_string(),
                    api_token: Some("glsa_stg_token_here".to_string()),
                    org_id: None,
                    folders: vec![], // Empty list = Query all folders
                },
            ],
            uptime_kuma: vec![
                UptimeKumaConfig {
                    name: "Uptime Kuma Cloud".to_string(),
                    url: "https://kuma.example.com".to_string(),
                    username: Some("admin".to_string()),
                    password: Some("secret".to_string()),
                    api_token: None,
                    monitor_groups: vec!["Production Web".to_string()],
                    tags: vec!["critical".to_string()],
                },
                UptimeKumaConfig {
                    name: "Uptime Kuma Edge (All Monitors)".to_string(),
                    url: "https://kuma-edge.example.com".to_string(),
                    username: None,
                    password: None,
                    api_token: None,
                    monitor_groups: vec![],
                    tags: vec![],
                },
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn get_state_path(app_handle: &tauri::AppHandle) -> PathBuf {
    if let Ok(config_dir) = app_handle.path().app_config_dir() {
        config_dir.join("state.yaml")
    } else {
        PathBuf::from("state.yaml")
    }
}

pub fn load_window_state(app_handle: &tauri::AppHandle) -> Option<WindowState> {
    let state_path = get_state_path(app_handle);
    if state_path.exists() {
        if let Ok(content) = fs::read_to_string(&state_path) {
            return serde_yaml::from_str(&content).ok();
        }
    }
    None
}

pub fn save_window_state(app_handle: &tauri::AppHandle, state: &WindowState) {
    let state_path = get_state_path(app_handle);
    if let Ok(yaml) = serde_yaml::to_string(state) {
        let _ = fs::write(state_path, yaml);
    }
}

pub fn get_config_path(app_handle: &tauri::AppHandle) -> PathBuf {
    if let Ok(config_dir) = app_handle.path().app_config_dir() {
        if !config_dir.exists() {
            let _ = fs::create_dir_all(&config_dir);
        }
        config_dir.join("config.yaml")
    } else {
        PathBuf::from("config.yaml")
    }
}

pub fn load_or_create_config(app_handle: &tauri::AppHandle) -> Result<AppConfig, String> {
    let config_path = get_config_path(app_handle);

    if !config_path.exists() {
        let default_config = AppConfig::default();
        let yaml_str = serde_yaml::to_string(&default_config)
            .map_err(|e| format!("Failed to serialize default config: {}", e))?;
        
        let header = "# Desktop Alert View Configuration File\n# Supports multiple instances of Zabbix, Grafana, and Uptime Kuma.\n# Leave host_groups, folders, monitor_groups, or tags empty ([]) to query ALL problems/alerts.\n\n";
        let full_yaml = format!("{}{}", header, yaml_str);

        fs::write(&config_path, full_yaml)
            .map_err(|e| format!("Failed to write sample config.yaml: {}", e))?;
        
        return Ok(default_config);
    }

    let content = fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config file {:?}: {}", config_path, e))?;

    let config: AppConfig = serde_yaml::from_str(&content)
        .map_err(|e| format!("Failed to parse YAML config file {:?}: {}", config_path, e))?;

    Ok(config)
}
