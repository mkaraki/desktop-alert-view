use crate::config::UptimeKumaConfig;
use crate::models::AlertItem;
use reqwest::Client;
use serde_json::Value;

pub async fn fetch_uptime_kuma_alerts(
    client: &Client,
    config: &UptimeKumaConfig,
) -> Result<Vec<AlertItem>, String> {
    let base_url = config.url.trim_end_matches('/');
    let mut status_reasons: Vec<String> = Vec::new();

    // 1. Try /metrics endpoint with Basic Auth (curl -u":<api-key>" style or user:pass)
    let metrics_url = format!("{}/metrics", base_url);
    let mut req_metrics = client.get(&metrics_url);

    if let Some(token) = &config.api_token {
        if !token.trim().is_empty() {
            // curl -u":<api-key>" passes empty username and api_key as password
            req_metrics = req_metrics.basic_auth("", Some(token.trim()));
        }
    } else if let (Some(user), Some(pass)) = (&config.username, &config.password) {
        if !user.is_empty() {
            req_metrics = req_metrics.basic_auth(user, Some(pass));
        }
    }

    match req_metrics.send().await {
        Ok(metrics_res) => {
            let status = metrics_res.status();
            if status.is_success() {
                if let Ok(text) = metrics_res.text().await {
                    return parse_metrics_alerts(&text, config);
                }
            } else {
                status_reasons.push(format!(
                    "/metrics HTTP {} ({})",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Unknown")
                ));
            }
        }
        Err(e) => {
            status_reasons.push(format_reqwest_err("/metrics", &e));
        }
    }

    // 2. Fallback: Try status page manifest API
    let manifest_url = format!("{}/api/status-page/manifest/default", base_url);
    let heartbeat_url = format!("{}/api/status-page/heartbeat/default", base_url);

    let mut req_manifest = client.get(&manifest_url);
    if let Some(token) = &config.api_token {
        if !token.trim().is_empty() {
            req_manifest = req_manifest.basic_auth("", Some(token.trim()));
        }
    } else if let (Some(user), Some(pass)) = (&config.username, &config.password) {
        if !user.is_empty() {
            req_manifest = req_manifest.basic_auth(user, Some(pass));
        }
    }

    match req_manifest.send().await {
        Ok(manifest_res) => {
            let status = manifest_res.status();
            if status.is_success() {
                if let Ok(manifest_json) = manifest_res.json::<Value>().await {
                    let mut req_hb = client.get(&heartbeat_url);
                    if let Some(token) = &config.api_token {
                        if !token.trim().is_empty() {
                            req_hb = req_hb.basic_auth("", Some(token.trim()));
                        }
                    } else if let (Some(user), Some(pass)) = (&config.username, &config.password) {
                        if !user.is_empty() {
                            req_hb = req_hb.basic_auth(user, Some(pass));
                        }
                    }

                    if let Ok(hb_res) = req_hb.send().await {
                        if hb_res.status().is_success() {
                            if let Ok(hb_json) = hb_res.json::<Value>().await {
                                return parse_status_page_alerts(manifest_json, hb_json, config);
                            }
                        }
                    }
                }
            } else {
                status_reasons.push(format!(
                    "status-page manifest HTTP {} ({})",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Unknown")
                ));
            }
        }
        Err(e) => {
            status_reasons.push(format_reqwest_err("status-page manifest", &e));
        }
    }

    let detail = if !status_reasons.is_empty() {
        status_reasons.join(", ")
    } else {
        "Unknown error".to_string()
    };

    Err(format!(
        "Uptime Kuma [{}]: Failed to query metrics or status page ({})",
        config.name, detail
    ))
}

fn format_reqwest_err(action: &str, e: &reqwest::Error) -> String {
    if let Some(status) = e.status() {
        format!(
            "{} HTTP {} ({})",
            action,
            status.as_u16(),
            status.canonical_reason().unwrap_or("Unknown")
        )
    } else {
        format!("{}: {}", action, e)
    }
}

fn parse_metrics_alerts(
    metrics_text: &str,
    config: &UptimeKumaConfig,
) -> Result<Vec<AlertItem>, String> {
    let mut alerts = Vec::new();
    let base_url = config.url.trim_end_matches('/');

    for line in metrics_text.lines() {
        if line.starts_with("monitor_status{") {
            // Check if status is 0 (DOWN)
            if line.ends_with(" 0") || line.contains("} 0") {
                let name = extract_label_val(line, "monitor_name").unwrap_or_else(|| "Monitor".to_string());
                let host = extract_label_val(line, "monitor_hostname")
                    .or_else(|| extract_label_val(line, "monitor_url"))
                    .unwrap_or_else(|| name.clone());

                // Filter by monitor_groups if specified
                if !config.monitor_groups.is_empty() {
                    let group_label = extract_label_val(line, "monitor_group").unwrap_or_default();
                    if !group_label.is_empty()
                        && !config
                            .monitor_groups
                            .iter()
                            .any(|g| g.eq_ignore_ascii_case(&group_label))
                    {
                        continue;
                    }
                }

                // Filter by tags if specified
                if !config.tags.is_empty() {
                    let tag_label = extract_label_val(line, "monitor_tags").unwrap_or_default();
                    if !tag_label.is_empty()
                        && !config
                            .tags
                            .iter()
                            .any(|t| tag_label.to_lowercase().contains(&t.to_lowercase()))
                    {
                        continue;
                    }
                }

                let jump_url = format!("{}/dashboard", base_url);

                alerts.push(AlertItem {
                    id: format!("kuma_metric_{}_{}", config.name, name),
                    source_name: config.name.clone(),
                    source_type: "uptime_kuma".to_string(),
                    title: format!("Monitor Down: {}", name),
                    hostname: extract_clean_host(&host),
                    severity: "down".to_string(),
                    status: "firing".to_string(),
                    timestamp: "Down".to_string(),
                    jump_url,
                    acknowledged: true,
                });
            }
        }
    }

    Ok(alerts)
}

fn parse_status_page_alerts(
    manifest: Value,
    heartbeats: Value,
    config: &UptimeKumaConfig,
) -> Result<Vec<AlertItem>, String> {
    let mut alerts = Vec::new();
    let base_url = config.url.trim_end_matches('/');

    let heartbeat_list = heartbeats.get("heartbeatList").and_then(|h| h.as_object());

    if let Some(group_list) = manifest
        .get("config")
        .and_then(|c| c.get("groupList"))
        .and_then(|g| g.as_array())
    {
        for group in group_list {
            let group_name = group.get("name").and_then(|n| n.as_str()).unwrap_or("");

            if !config.monitor_groups.is_empty() {
                if !config
                    .monitor_groups
                    .iter()
                    .any(|g| g.eq_ignore_ascii_case(group_name))
                {
                    continue;
                }
            }

            if let Some(monitor_list) = group.get("monitorList").and_then(|m| m.as_array()) {
                for monitor in monitor_list {
                    let id_val = monitor.get("id");
                    let id_str = match id_val {
                        Some(Value::Number(n)) => n.to_string(),
                        Some(Value::String(s)) => s.clone(),
                        _ => continue,
                    };

                    let monitor_name = monitor
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("Monitor");

                    let url_or_host = monitor
                        .get("url")
                        .or_else(|| monitor.get("hostname"))
                        .and_then(|u| u.as_str())
                        .unwrap_or(monitor_name);

                    if !config.tags.is_empty() {
                        let has_matching_tag = if let Some(tags) =
                            monitor.get("tags").and_then(|t| t.as_array())
                        {
                            tags.iter().any(|t| {
                                let tag_name = t
                                    .get("name")
                                    .or_else(|| t.as_str().map(|_| t))
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("");
                                config.tags.iter().any(|ct| ct.eq_ignore_ascii_case(tag_name))
                            })
                        } else {
                            false
                        };

                        if !has_matching_tag {
                            continue;
                        }
                    }

                    if let Some(hb_obj) = heartbeat_list {
                        if let Some(hb_arr) = hb_obj.get(&id_str).and_then(|a| a.as_array()) {
                            if let Some(last_hb) = hb_arr.last() {
                                let status_code = last_hb
                                    .get("status")
                                    .and_then(|s| s.as_i64())
                                    .unwrap_or(1);

                                if status_code == 0 {
                                    let msg = last_hb
                                        .get("msg")
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("Service Down / Connection Error");
                                    let time = last_hb
                                        .get("time")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");

                                    let jump_url = format!("{}/dashboard/{}", base_url, id_str);

                                    alerts.push(AlertItem {
                                        id: format!("kuma_{}_{}", config.name, id_str),
                                        source_name: config.name.clone(),
                                        source_type: "uptime_kuma".to_string(),
                                        title: format!("{}: {}", monitor_name, msg),
                                        hostname: extract_clean_host(url_or_host),
                                        severity: "down".to_string(),
                                        status: "firing".to_string(),
                                        timestamp: time.to_string(),
                                        jump_url,
                                        acknowledged: true,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(alerts)
}

fn extract_label_val(line: &str, label: &str) -> Option<String> {
    let key = format!("{}=\"", label);
    if let Some(start_idx) = line.find(&key) {
        let val_start = start_idx + key.len();
        if let Some(end_idx) = line[val_start..].find('"') {
            return Some(line[val_start..val_start + end_idx].to_string());
        }
    }
    None
}

fn extract_clean_host(target: &str) -> String {
    if target.starts_with("http://") || target.starts_with("https://") {
        if let Ok(parsed) = url::Url::parse(target) {
            if let Some(host) = parsed.host_str() {
                return host.to_string();
            }
        }
    }
    target.to_string()
}
