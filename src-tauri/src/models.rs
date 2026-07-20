use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertItem {
    pub id: String,
    pub source_name: String,
    pub source_type: String, // "zabbix" | "grafana" | "uptime_kuma"
    pub title: String,
    pub hostname: String,
    pub severity: String,   // "disaster" | "high" | "warning" | "info" | "down" | "resolved_unacked"
    pub status: String,     // "firing" | "resolved_unacked"
    pub timestamp: String,
    pub jump_url: String,
    pub acknowledged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertFetchResult {
    pub active_alerts: Vec<AlertItem>,
    pub closed_unacked_alerts: Vec<AlertItem>,
    pub errors: Vec<String>,
}
