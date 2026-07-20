use crate::config::GrafanaConfig;
use crate::models::AlertItem;
use reqwest::Client;
use serde_json::Value;

pub async fn fetch_grafana_alerts(
    client: &Client,
    config: &GrafanaConfig,
) -> Result<(Vec<AlertItem>, Vec<AlertItem>), String> {
    let base_url = config.url.trim_end_matches('/');
    let mut active_alerts = Vec::new();
    let mut resolved_alerts = Vec::new();
    let mut last_error: Option<String> = None;

    // 1. Fetch active firing alerts from Alertmanager v2
    let alertmanager_url = format!("{}/api/alertmanager/grafana/api/v2/alerts", base_url);
    let mut req = client.get(&alertmanager_url);
    if let Some(token) = &config.api_token {
        if !token.trim().is_empty() {
            req = req.header("Authorization", format!("Bearer {}", token.trim()));
        }
    }
    if let Some(org_id) = config.org_id {
        req = req.header("X-Grafana-Org-Id", org_id.to_string());
    }

    if let Ok(resp) = req.send().await {
        if resp.status().is_success() {
            if let Ok(alerts_json) = resp.json::<Value>().await {
                if let Ok((act, res)) = parse_alertmanager_alerts(alerts_json, config) {
                    active_alerts.extend(act);
                    resolved_alerts.extend(res);
                }
            }
        } else {
            last_error = Some(format!(
                "HTTP {} ({})",
                resp.status().as_u16(),
                resp.status().canonical_reason().unwrap_or("Unknown")
            ));
        }
    }

    // 2. Fetch Grafana Annotations API strictly for Resolved alert history (OK / Normal state events)
    let annotations_url = format!("{}/api/annotations?limit=100", base_url);
    let mut req_anno = client.get(&annotations_url);
    if let Some(token) = &config.api_token {
        if !token.trim().is_empty() {
            req_anno = req_anno.header("Authorization", format!("Bearer {}", token.trim()));
        }
    }
    if let Some(org_id) = config.org_id {
        req_anno = req_anno.header("X-Grafana-Org-Id", org_id.to_string());
    }

    if let Ok(resp_anno) = req_anno.send().await {
        if resp_anno.status().is_success() {
            if let Ok(anno_json) = resp_anno.json::<Value>().await {
                let anno_res = parse_annotations_history(anno_json, config);
                for item in anno_res {
                    if !resolved_alerts.iter().any(|a| a.id == item.id) {
                        resolved_alerts.push(item);
                    }
                }
            }
        }
    }

    // 3. Fallback: Prometheus alerts endpoint if active_alerts is empty
    if active_alerts.is_empty() {
        let prom_url = format!("{}/api/prometheus/grafana/api/v1/alerts", base_url);
        let mut req_prom = client.get(&prom_url);
        if let Some(token) = &config.api_token {
            if !token.trim().is_empty() {
                req_prom = req_prom.header("Authorization", format!("Bearer {}", token.trim()));
            }
        }
        if let Some(org_id) = config.org_id {
            req_prom = req_prom.header("X-Grafana-Org-Id", org_id.to_string());
        }

        if let Ok(resp) = req_prom.send().await {
            if resp.status().is_success() {
                if let Ok(prom_json) = resp.json::<Value>().await {
                    if let Ok((act, res)) = parse_prometheus_alerts(prom_json, config) {
                        active_alerts.extend(act);
                        resolved_alerts.extend(res);
                    }
                }
            }
        }
    }

    // 4. Fallback: Legacy Grafana alerts endpoint if still empty
    if active_alerts.is_empty() && resolved_alerts.is_empty() {
        let legacy_url = format!("{}/api/alerts?state=ALL", base_url);
        let mut req_legacy = client.get(&legacy_url);
        if let Some(token) = &config.api_token {
            if !token.trim().is_empty() {
                req_legacy = req_legacy.header("Authorization", format!("Bearer {}", token.trim()));
            }
        }
        if let Some(org_id) = config.org_id {
            req_legacy = req_legacy.header("X-Grafana-Org-Id", org_id.to_string());
        }

        if let Ok(resp) = req_legacy.send().await {
            let status = resp.status();
            if status.is_success() {
                if let Ok(legacy_json) = resp.json::<Value>().await {
                    let (act, res) = parse_legacy_alerts(legacy_json, config)?;
                    active_alerts.extend(act);
                    resolved_alerts.extend(res);
                }
            } else {
                return Err(format!(
                    "Grafana [{}]: HTTP {} ({})",
                    config.name,
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Unknown")
                ));
            }
        } else if let Some(err_msg) = last_error {
            return Err(format!("Grafana [{}]: {}", config.name, err_msg));
        }
    }

    Ok((active_alerts, resolved_alerts))
}

fn parse_annotations_history(
    json: Value,
    config: &GrafanaConfig,
) -> Vec<AlertItem> {
    let mut resolved = Vec::new();
    let base_url = config.url.trim_end_matches('/');

    if let Some(list) = json.as_array() {
        for item in list {
            let id = item.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            let new_state = item.get("newState").and_then(|v| v.as_str()).unwrap_or("");
            let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let alert_name = item.get("alertName").or_else(|| item.get("title")).and_then(|v| v.as_str()).unwrap_or("Grafana Alert");

            // Annotations history is strictly for resolved history (OK / Normal)
            if !new_state.eq_ignore_ascii_case("OK") && !new_state.eq_ignore_ascii_case("Normal") {
                continue;
            }

            if !config.folders.is_empty() {
                let text_lower = text.to_lowercase();
                let matches_folder = config.folders.iter().any(|f| text_lower.contains(&f.to_lowercase()));
                if !matches_folder {
                    continue;
                }
            }

            let timestamp = if let Some(ms) = item.get("time").and_then(|v| v.as_i64()) {
                format_epoch_ms(ms)
            } else {
                "Unknown".to_string()
            };

            let hostname = extract_host_from_annotation_text(text);
            
            // Build URL: always use /alerting/grafana/:id/view for alert rule history view
            let alert_view_id = extract_alert_view_id(item);

            let jump_url = if let Some(uid) = alert_view_id {
                format!("{}/alerting/grafana/{}/view", base_url, uid)
            } else {
                format!("{}/alerting/list", base_url)
            };

            resolved.push(AlertItem {
                id: format!("grafana_anno_{}_{}", config.name, id),
                source_name: config.name.clone(),
                source_type: "grafana".to_string(),
                title: format!("[RESOLVED] {}", alert_name),
                hostname,
                severity: "resolved_unacked".to_string(),
                status: "resolved_unacked".to_string(),
                timestamp,
                jump_url,
                acknowledged: false,
            });
        }
    }

    resolved
}

fn extract_alert_view_id(item: &Value) -> Option<String> {
    // 1. Check data.ruleUid or data.rule_uid or data.alertRuleUid
    if let Some(data) = item.get("data") {
        if let Some(uid) = data
            .get("ruleUid")
            .or_else(|| data.get("rule_uid"))
            .or_else(|| data.get("alertRuleUid"))
            .and_then(|v| v.as_str())
        {
            if !uid.is_empty() {
                return Some(uid.to_string());
            }
        }
    }

    // 2. Check dashboardUID
    if let Some(dash_uid) = item.get("dashboardUID").and_then(|v| v.as_str()) {
        if !dash_uid.is_empty() {
            return Some(dash_uid.to_string());
        }
    }

    // 3. Search text for rule_uid or alert_uid
    let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
    for key in &["rule_uid=", "ruleUid=", "alert_uid=", "alertRuleUid="] {
        if let Some(idx) = text.find(key) {
            let start = idx + key.len();
            let end = text[start..].find(|c: char| c == ',' || c == '}' || c == ' ' || c == '"').unwrap_or(text[start..].len());
            let val = text[start..start + end].trim_matches('"').trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }

    // 4. Check alertId
    if let Some(alert_id) = item.get("alertId") {
        let val_str = match alert_id {
            Value::Number(n) => n.to_string(),
            Value::String(s) if !s.is_empty() => s.clone(),
            _ => String::new(),
        };
        if !val_str.is_empty() {
            return Some(val_str);
        }
    }

    None
}

fn extract_host_from_annotation_text(text: &str) -> String {
    for label in &["host=", "instance=", "node=", "cluster="] {
        if let Some(idx) = text.find(label) {
            let start = idx + label.len();
            let end = text[start..].find(|c: char| c == ',' || c == '}' || c == ' ' || c == '\n').unwrap_or(text[start..].len());
            let val = text[start..start + end].trim_matches('"').trim();
            if !val.is_empty() {
                return val.to_string();
            }
        }
    }
    "N/A".to_string()
}

fn parse_alertmanager_alerts(
    json: Value,
    config: &GrafanaConfig,
) -> Result<(Vec<AlertItem>, Vec<AlertItem>), String> {
    let mut active_items = Vec::new();
    let mut resolved_items = Vec::new();
    let base_url = config.url.trim_end_matches('/');

    if let Some(alert_list) = json.as_array() {
        for (idx, alert) in alert_list.iter().enumerate() {
            let status_state = alert
                .get("status")
                .and_then(|s| s.get("state"))
                .and_then(|s| s.as_str())
                .unwrap_or("active");

            let labels = alert.get("labels");
            let annotations = alert.get("annotations");

            if !config.folders.is_empty() {
                let folder_name = labels
                    .and_then(|l| l.get("grafana_folder"))
                    .and_then(|f| f.as_str())
                    .or_else(|| {
                        annotations
                            .and_then(|a| a.get("folder"))
                            .and_then(|f| f.as_str())
                    })
                    .unwrap_or("");

                if !folder_name.is_empty()
                    && !config
                        .folders
                        .iter()
                        .any(|f| f.eq_ignore_ascii_case(folder_name))
                {
                    continue;
                }
            }

            let title_raw = annotations
                .and_then(|a| a.get("summary").or_else(|| a.get("title")))
                .and_then(|v| v.as_str())
                .or_else(|| labels.and_then(|l| l.get("alertname")).and_then(|v| v.as_str()))
                .unwrap_or("Grafana Alert");

            let hostname = labels
                .and_then(|l| {
                    l.get("instance")
                        .or_else(|| l.get("host"))
                        .or_else(|| l.get("hostname"))
                        .or_else(|| l.get("cluster"))
                        .or_else(|| l.get("node"))
                })
                .and_then(|v| v.as_str())
                .unwrap_or("N/A");

            let starts_at = alert
                .get("startsAt")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let timestamp = format_iso_time(starts_at);

            let rule_uid = labels
                .and_then(|l| l.get("rule_uid").or_else(|| l.get("grafana_alert_id")))
                .and_then(|v| v.as_str())
                .or_else(|| annotations.and_then(|a| a.get("__alertRuleUid__")).and_then(|v| v.as_str()));

            let jump_url = if let Some(gen_url) = alert.get("generatorURL").and_then(|v| v.as_str()) {
                if !gen_url.is_empty() {
                    gen_url.to_string()
                } else if let Some(ruid) = rule_uid {
                    format!("{}/alerting/grafana/{}/view", base_url, ruid)
                } else {
                    format!("{}/alerting/list", base_url)
                }
            } else if let Some(ruid) = rule_uid {
                format!("{}/alerting/grafana/{}/view", base_url, ruid)
            } else {
                format!("{}/alerting/list", base_url)
            };

            let fingerprint = alert
                .get("fingerprint")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("{}", idx));

            if status_state == "firing" || status_state == "active" {
                let severity_raw = labels
                    .and_then(|l| l.get("severity"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("warning");

                active_items.push(AlertItem {
                    id: format!("grafana_{}_{}", config.name, fingerprint),
                    source_name: config.name.clone(),
                    source_type: "grafana".to_string(),
                    title: title_raw.to_string(),
                    hostname: hostname.to_string(),
                    severity: map_grafana_severity(severity_raw),
                    status: "firing".to_string(),
                    timestamp,
                    jump_url,
                    acknowledged: true,
                });
            } else if status_state == "resolved" {
                resolved_items.push(AlertItem {
                    id: format!("grafana_resolved_{}_{}", config.name, fingerprint),
                    source_name: config.name.clone(),
                    source_type: "grafana".to_string(),
                    title: format!("[RESOLVED] {}", title_raw),
                    hostname: hostname.to_string(),
                    severity: "resolved_unacked".to_string(),
                    status: "resolved_unacked".to_string(),
                    timestamp,
                    jump_url,
                    acknowledged: false,
                });
            }
        }
    }

    Ok((active_items, resolved_items))
}

fn parse_prometheus_alerts(
    json: Value,
    config: &GrafanaConfig,
) -> Result<(Vec<AlertItem>, Vec<AlertItem>), String> {
    let mut active_items = Vec::new();
    let mut resolved_items = Vec::new();
    let base_url = config.url.trim_end_matches('/');

    if let Some(alerts) = json.get("data").and_then(|d| d.get("alerts")).and_then(|a| a.as_array()) {
        for (idx, alert) in alerts.iter().enumerate() {
            let state = alert.get("state").and_then(|v| v.as_str()).unwrap_or("");
            let labels = alert.get("labels");
            let annotations = alert.get("annotations");

            if !config.folders.is_empty() {
                let folder_name = labels
                    .and_then(|l| l.get("grafana_folder"))
                    .and_then(|f| f.as_str())
                    .unwrap_or("");
                if !folder_name.is_empty()
                    && !config
                        .folders
                        .iter()
                        .any(|f| f.eq_ignore_ascii_case(folder_name))
                {
                    continue;
                }
            }

            let title_raw = annotations
                .and_then(|a| a.get("summary").or_else(|| a.get("title")))
                .and_then(|v| v.as_str())
                .or_else(|| labels.and_then(|l| l.get("alertname")).and_then(|v| v.as_str()))
                .unwrap_or("Grafana Alert");

            let hostname = labels
                .and_then(|l| {
                    l.get("instance")
                        .or_else(|| l.get("host"))
                        .or_else(|| l.get("cluster"))
                })
                .and_then(|v| v.as_str())
                .unwrap_or("N/A");

            let active_at = alert.get("activeAt").and_then(|v| v.as_str()).unwrap_or("");
            let timestamp = format_iso_time(active_at);

            let rule_uid = labels
                .and_then(|l| l.get("rule_uid").or_else(|| l.get("grafana_alert_id")))
                .and_then(|v| v.as_str());

            let jump_url = if let Some(ruid) = rule_uid {
                format!("{}/alerting/grafana/{}/view", base_url, ruid)
            } else {
                format!("{}/alerting/list", base_url)
            };

            if state == "firing" || state == "alerting" {
                let severity_raw = labels
                    .and_then(|l| l.get("severity"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("warning");

                active_items.push(AlertItem {
                    id: format!("grafana_prom_{}_{}", config.name, idx),
                    source_name: config.name.clone(),
                    source_type: "grafana".to_string(),
                    title: title_raw.to_string(),
                    hostname: hostname.to_string(),
                    severity: map_grafana_severity(severity_raw),
                    status: "firing".to_string(),
                    timestamp,
                    jump_url,
                    acknowledged: true,
                });
            } else if state == "resolved" || state == "inactive" {
                resolved_items.push(AlertItem {
                    id: format!("grafana_prom_res_{}_{}", config.name, idx),
                    source_name: config.name.clone(),
                    source_type: "grafana".to_string(),
                    title: format!("[RESOLVED] {}", title_raw),
                    hostname: hostname.to_string(),
                    severity: "resolved_unacked".to_string(),
                    status: "resolved_unacked".to_string(),
                    timestamp,
                    jump_url,
                    acknowledged: false,
                });
            }
        }
    }

    Ok((active_items, resolved_items))
}

fn parse_legacy_alerts(
    json: Value,
    config: &GrafanaConfig,
) -> Result<(Vec<AlertItem>, Vec<AlertItem>), String> {
    let mut active_items = Vec::new();
    let mut resolved_items = Vec::new();
    let base_url = config.url.trim_end_matches('/');

    if let Some(list) = json.as_array() {
        for alert in list {
            let id = alert.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            let name = alert.get("name").and_then(|v| v.as_str()).unwrap_or("Grafana Alert");
            let state = alert.get("state").and_then(|v| v.as_str()).unwrap_or("");

            let jump_url = format!("{}/alerting/grafana/{}/view", base_url, id);

            let eval_date = alert.get("newStateDate").and_then(|v| v.as_str()).unwrap_or("");
            let timestamp = format_iso_time(eval_date);

            if state == "alerting" {
                active_items.push(AlertItem {
                    id: format!("grafana_legacy_{}_{}", config.name, id),
                    source_name: config.name.clone(),
                    source_type: "grafana".to_string(),
                    title: name.to_string(),
                    hostname: "N/A".to_string(),
                    severity: "high".to_string(),
                    status: "firing".to_string(),
                    timestamp,
                    jump_url,
                    acknowledged: true,
                });
            } else if state == "ok" || state == "paused" {
                resolved_items.push(AlertItem {
                    id: format!("grafana_legacy_ok_{}_{}", config.name, id),
                    source_name: config.name.clone(),
                    source_type: "grafana".to_string(),
                    title: format!("[RESOLVED] {}", name),
                    hostname: "N/A".to_string(),
                    severity: "resolved_unacked".to_string(),
                    status: "resolved_unacked".to_string(),
                    timestamp,
                    jump_url,
                    acknowledged: false,
                });
            }
        }
    }

    Ok((active_items, resolved_items))
}

fn map_grafana_severity(sev: &str) -> String {
    match sev.to_lowercase().as_str() {
        "critical" | "disaster" | "fatal" | "page" => "disaster".to_string(),
        "error" | "high" => "high".to_string(),
        "warn" | "warning" | "medium" => "warning".to_string(),
        "info" | "information" | "low" => "info".to_string(),
        _ => "warning".to_string(),
    }
}

fn format_iso_time(iso_str: &str) -> String {
    if iso_str.is_empty() {
        return "Unknown".to_string();
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso_str) {
        return dt.format("%Y-%m-%d %H:%M:%S").to_string();
    }
    iso_str.chars().take(19).collect::<String>().replace("T", " ")
}

fn format_epoch_ms(ms: i64) -> String {
    let secs = ms / 1000;
    if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
        return dt.format("%Y-%m-%d %H:%M:%S").to_string();
    }
    "Unknown".to_string()
}
