use crate::config::ZabbixConfig;
use crate::models::AlertItem;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
struct TriggerMeta {
    pub hostname: String,
    pub is_monitored: bool,
    pub dependency_trigger_ids: Vec<String>,
}

pub async fn fetch_zabbix_alerts(
    client: &Client,
    config: &ZabbixConfig,
) -> Result<(Vec<AlertItem>, Vec<AlertItem>), String> {
    let base_url = config.url.trim_end_matches('/');
    let api_url = format!("{}/api_jsonrpc.php", base_url);

    // 1. Authenticate if username/password provided and no api_token
    let auth_token: Option<String> = if let Some(token) = &config.api_token {
        if !token.trim().is_empty() {
            Some(token.trim().to_string())
        } else {
            authenticate_zabbix(client, &api_url, config).await?
        }
    } else {
        authenticate_zabbix(client, &api_url, config).await?
    };

    // 2. Resolve Host Group names to Group IDs with case-insensitive matching
    let group_ids: Option<Vec<String>> = if !config.host_groups.is_empty() {
        let ids = fetch_host_group_ids(client, &api_url, auth_token.as_deref(), config).await?;
        if ids.is_empty() {
            return Err(format!(
                "Zabbix [{}]: Configured host group(s) {:?} not found on Zabbix server",
                config.name, config.host_groups
            ));
        }
        Some(ids)
    } else {
        None
    };

    // 3. Fetch Active Problems (unacknowledged, unsuppressed, and trigger dependency filtered)
    let active_alerts = fetch_active_problems(client, &api_url, auth_token.as_deref(), config, group_ids.as_ref()).await?;

    // 4. Fetch Closed but Unacknowledged & Unsuppressed Alerts History
    let closed_unacked = fetch_closed_unacknowledged_events(client, &api_url, auth_token.as_deref(), config, group_ids.as_ref()).await?;

    Ok((active_alerts, closed_unacked))
}

async fn authenticate_zabbix(
    client: &Client,
    api_url: &str,
    config: &ZabbixConfig,
) -> Result<Option<String>, String> {
    let username = config.username.as_deref().unwrap_or("");
    let password = config.password.as_deref().unwrap_or("");

    if username.is_empty() {
        return Ok(None);
    }

    let payload = json!({
        "jsonrpc": "2.0",
        "method": "user.login",
        "params": {
            "username": username,
            "password": password
        },
        "id": 1
    });

    let resp = client
        .post(api_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format_error(&config.name, "user.login request error", &e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Zabbix [{}] HTTP {} ({}) during user.login",
            config.name,
            resp.status().as_u16(),
            resp.status().canonical_reason().unwrap_or("Unknown")
        ));
    }

    let resp_json: Value = resp
        .json()
        .await
        .map_err(|e| format!("Zabbix [{}] user.login JSON parse error: {}", config.name, e))?;

    if let Some(result) = resp_json.get("result").and_then(|r| r.as_str()) {
        Ok(Some(result.to_string()))
    } else if let Some(error) = resp_json.get("error") {
        let msg = error.get("message").and_then(|m| m.as_str()).unwrap_or("Login error");
        let data = error.get("data").and_then(|d| d.as_str()).unwrap_or("");
        Err(format!("Zabbix [{}] user.login failed: {} ({})", config.name, msg, data))
    } else {
        Err(format!("Zabbix [{}] user.login failed with unknown response structure", config.name))
    }
}

async fn fetch_host_group_ids(
    client: &Client,
    api_url: &str,
    auth_token: Option<&str>,
    config: &ZabbixConfig,
) -> Result<Vec<String>, String> {
    let params = json!({
        "output": ["groupid", "name"],
        "monitored_hosts": true
    });

    let payload = make_jsonrpc_payload("hostgroup.get", params, auth_token);

    let mut req = client.post(api_url).json(&payload);
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {}", token));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format_error(&config.name, "hostgroup.get request error", &e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Zabbix [{}] HTTP {} ({}) during hostgroup.get",
            config.name,
            resp.status().as_u16(),
            resp.status().canonical_reason().unwrap_or("Unknown")
        ));
    }

    let resp_json: Value = resp
        .json()
        .await
        .map_err(|e| format!("Zabbix [{}] hostgroup.get JSON parse error: {}", config.name, e))?;

    if let Some(error) = resp_json.get("error") {
        let msg = error.get("message").and_then(|m| m.as_str()).unwrap_or("API error");
        let data = error.get("data").and_then(|d| d.as_str()).unwrap_or("");
        return Err(format!("Zabbix [{}] hostgroup.get API error: {} ({})", config.name, msg, data));
    }

    let mut ids = Vec::new();
    if let Some(result) = resp_json.get("result").and_then(|r| r.as_array()) {
        for group in result {
            if let (Some(gid), Some(gname)) = (
                group.get("groupid").and_then(|g| g.as_str()),
                group.get("name").and_then(|n| n.as_str()),
            ) {
                for target_group in &config.host_groups {
                    if target_group.eq_ignore_ascii_case(gname) {
                        ids.push(gid.to_string());
                        break;
                    }
                }
            }
        }
    }

    Ok(ids)
}

async fn fetch_active_problems(
    client: &Client,
    api_url: &str,
    auth_token: Option<&str>,
    config: &ZabbixConfig,
    group_ids: Option<&Vec<String>>,
) -> Result<Vec<AlertItem>, String> {
    // Query ALL active problems (including suppressed/acknowledged) to collect full firing trigger dependency set
    let mut params = json!({
        "output": ["eventid", "objectid", "name", "severity", "clock", "acknowledged", "suppressed"],
        "recent": false,
        "sortfield": ["eventid"],
        "sortorder": "DESC"
    });

    if let Some(gids) = group_ids {
        params["groupids"] = json!(gids);
    }

    let payload = make_jsonrpc_payload("problem.get", params, auth_token);

    let mut req = client.post(api_url).json(&payload);
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {}", token));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format_error(&config.name, "problem.get request error", &e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Zabbix [{}] HTTP {} ({}) during problem.get",
            config.name,
            resp.status().as_u16(),
            resp.status().canonical_reason().unwrap_or("Unknown")
        ));
    }

    let resp_json: Value = resp
        .json()
        .await
        .map_err(|e| format!("Zabbix [{}] problem.get JSON parse error: {}", config.name, e))?;

    if let Some(error) = resp_json.get("error") {
        let msg = error.get("message").and_then(|m| m.as_str()).unwrap_or("API error");
        let data = error.get("data").and_then(|d| d.as_str()).unwrap_or("");
        return Err(format!("Zabbix [{}]: problem.get API error: {} ({})", config.name, msg, data));
    }

    let mut all_firing_trigger_set = HashSet::new();
    let mut raw_items = Vec::new();
    let mut trigger_ids = Vec::new();

    if let Some(result) = resp_json.get("result").and_then(|r| r.as_array()) {
        for item in result {
            let event_id = item.get("eventid").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let trigger_id = item.get("objectid").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("Unknown Problem").to_string();
            let severity_num = item.get("severity").and_then(|v| v.as_str()).unwrap_or("0").to_string();
            let clock = item.get("clock").and_then(|v| v.as_str()).unwrap_or("0").to_string();
            let suppressed = item.get("suppressed").and_then(|v| v.as_str()).unwrap_or("0") == "1";
            let acknowledged = item.get("acknowledged").and_then(|v| v.as_str()).unwrap_or("0") == "1";

            // Record ALL firing trigger IDs for dependency evaluation (even if suppressed/acknowledged)
            if !trigger_id.is_empty() {
                all_firing_trigger_set.insert(trigger_id.clone());
                if !trigger_ids.contains(&trigger_id) {
                    trigger_ids.push(trigger_id.clone());
                }
            }

            // Keep only unsuppressed & unacknowledged problems for final UI display candidates
            if !suppressed && !acknowledged {
                raw_items.push((event_id, trigger_id, name, severity_num, clock, acknowledged));
            }
        }
    }

    let trigger_map = fetch_monitored_triggers(client, api_url, auth_token, config, &trigger_ids).await;

    let mut alerts = Vec::new();
    for (event_id, trigger_id, name, severity_num, clock, acknowledged) in raw_items {
        if let Some(meta) = trigger_map.get(&trigger_id) {
            if !meta.is_monitored {
                continue;
            }

            // Dependency check: if ANY parent trigger is firing in Zabbix (even if suppressed/acknowledged), suppress this dependent trigger!
            let is_dependency_active = meta
                .dependency_trigger_ids
                .iter()
                .any(|dep_id| all_firing_trigger_set.contains(dep_id));

            if is_dependency_active {
                // Hide dependent problem because a parent dependency trigger is currently active
                continue;
            }

            let severity = map_zabbix_severity(&severity_num);
            let base_url = config.url.trim_end_matches('/');

            let jump_url = format!(
                "{}/zabbix.php?action=problem.view&filter_set=1&filter_show=1&filter_triggerids%5B%5D={}&triggerids%5B%5D={}",
                base_url, trigger_id, trigger_id
            );

            alerts.push(AlertItem {
                id: format!("zabbix_{}_{}", config.name, event_id),
                source_name: config.name.clone(),
                source_type: "zabbix".to_string(),
                title: name,
                hostname: meta.hostname.clone(),
                severity,
                status: "firing".to_string(),
                timestamp: format_timestamp(&clock),
                jump_url,
                acknowledged,
            });
        }
    }

    Ok(alerts)
}

async fn fetch_closed_unacknowledged_events(
    client: &Client,
    api_url: &str,
    auth_token: Option<&str>,
    config: &ZabbixConfig,
    group_ids: Option<&Vec<String>>,
) -> Result<Vec<AlertItem>, String> {
    let mut params = json!({
        "output": ["eventid", "objectid", "name", "severity", "clock", "acknowledged", "suppressed", "value"],
        "value": [0],
        "acknowledged": false,
        "suppressed": false,
        "object": 0,
        "sortfield": ["clock"],
        "sortorder": "DESC"
    });

    if let Some(gids) = group_ids {
        params["groupids"] = json!(gids);
    }

    let payload = make_jsonrpc_payload("event.get", params, auth_token);

    let mut req = client.post(api_url).json(&payload);
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {}", token));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format_error(&config.name, "event.get request error", &e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Zabbix [{}] HTTP {} ({}) during event.get",
            config.name,
            resp.status().as_u16(),
            resp.status().canonical_reason().unwrap_or("Unknown")
        ));
    }

    let resp_json: Value = resp
        .json()
        .await
        .map_err(|e| format!("Zabbix [{}] event.get JSON parse error: {}", config.name, e))?;

    if let Some(error) = resp_json.get("error") {
        let msg = error.get("message").and_then(|m| m.as_str()).unwrap_or("API error");
        let data = error.get("data").and_then(|d| d.as_str()).unwrap_or("");
        return Err(format!("Zabbix [{}]: event.get API error: {} ({})", config.name, msg, data));
    }

    let mut raw_items = Vec::new();
    let mut trigger_ids = Vec::new();

    if let Some(result) = resp_json.get("result").and_then(|r| r.as_array()) {
        for item in result {
            let suppressed = item.get("suppressed").and_then(|v| v.as_str()).unwrap_or("0") == "1";
            let acknowledged = item.get("acknowledged").and_then(|v| v.as_str()).unwrap_or("0") == "1";

            if suppressed || acknowledged {
                continue;
            }

            let event_id = item.get("eventid").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let trigger_id = item.get("objectid").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("Resolved Event (Unacknowledged)").to_string();
            let clock = item.get("clock").and_then(|v| v.as_str()).unwrap_or("0").to_string();

            if !trigger_id.is_empty() && !trigger_ids.contains(&trigger_id) {
                trigger_ids.push(trigger_id.clone());
            }

            raw_items.push((event_id, trigger_id, name, clock));
        }
    }

    let trigger_map = fetch_monitored_triggers(client, api_url, auth_token, config, &trigger_ids).await;

    let mut alerts = Vec::new();
    for (event_id, trigger_id, name, clock) in raw_items {
        if let Some(meta) = trigger_map.get(&trigger_id) {
            if !meta.is_monitored {
                continue;
            }

            let base_url = config.url.trim_end_matches('/');
            let jump_url = format!(
                "{}/zabbix.php?action=problem.view&filter_set=1&filter_show=1&filter_triggerids%5B%5D={}&triggerids%5B%5D={}",
                base_url, trigger_id, trigger_id
            );

            alerts.push(AlertItem {
                id: format!("zabbix_unacked_{}_{}", config.name, event_id),
                source_name: config.name.clone(),
                source_type: "zabbix".to_string(),
                title: format!("[RESOLVED] {}", name),
                hostname: meta.hostname.clone(),
                severity: "resolved_unacked".to_string(),
                status: "resolved_unacked".to_string(),
                timestamp: format_timestamp(&clock),
                jump_url,
                acknowledged: false,
            });
        }
    }

    Ok(alerts)
}

async fn fetch_monitored_triggers(
    client: &Client,
    api_url: &str,
    auth_token: Option<&str>,
    _config: &ZabbixConfig,
    trigger_ids: &[String],
) -> HashMap<String, TriggerMeta> {
    let mut map = HashMap::new();
    if trigger_ids.is_empty() {
        return map;
    }

    let params = json!({
        "triggerids": trigger_ids,
        "output": ["triggerid", "status"],
        "selectHosts": ["hostid", "host", "name", "status"],
        "selectDependencies": ["triggerid"]
    });

    let payload = make_jsonrpc_payload("trigger.get", params, auth_token);
    let mut req = client.post(api_url).json(&payload);
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {}", token));
    }

    if let Ok(resp) = req.send().await {
        if resp.status().is_success() {
            if let Ok(resp_json) = resp.json::<Value>().await {
                if let Some(result) = resp_json.get("result").and_then(|r| r.as_array()) {
                    for item in result {
                        let trigger_id = item.get("triggerid").and_then(|v| v.as_str()).unwrap_or("");
                        let trigger_status = item.get("status").and_then(|v| v.as_str()).unwrap_or("0");

                        let is_trigger_enabled = trigger_status == "0";
                        let (hostname, is_host_enabled) = extract_host_info(item);

                        let is_monitored = is_trigger_enabled && is_host_enabled;

                        let mut dep_ids = Vec::new();
                        if let Some(deps) = item.get("dependencies").and_then(|d| d.as_array()) {
                            for dep in deps {
                                if let Some(dep_tid) = dep.get("triggerid").and_then(|v| v.as_str()) {
                                    if !dep_tid.is_empty() {
                                        dep_ids.push(dep_tid.to_string());
                                    }
                                }
                            }
                        }

                        if !trigger_id.is_empty() {
                            map.insert(
                                trigger_id.to_string(),
                                TriggerMeta {
                                    hostname,
                                    is_monitored,
                                    dependency_trigger_ids: dep_ids,
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    map
}

fn extract_host_info(item: &Value) -> (String, bool) {
    if let Some(hosts) = item.get("hosts").and_then(|h| h.as_array()) {
        if let Some(first_host) = hosts.first() {
            let host_status = first_host.get("status").and_then(|s| s.as_str()).unwrap_or("0");
            let is_host_enabled = host_status == "0";

            let name = first_host
                .get("name")
                .and_then(|n| n.as_str())
                .filter(|n| !n.is_empty())
                .or_else(|| first_host.get("host").and_then(|h| h.as_str()))
                .unwrap_or("N/A");

            return (name.to_string(), is_host_enabled);
        }
    }
    ("N/A".to_string(), true)
}

fn make_jsonrpc_payload(method: &str, params: Value, auth_token: Option<&str>) -> Value {
    let mut payload = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1
    });

    if let Some(token) = auth_token {
        payload["auth"] = json!(token);
    }

    payload
}

fn format_error(source_name: &str, action: &str, err: &reqwest::Error) -> String {
    if let Some(status) = err.status() {
        format!(
            "Zabbix [{}]: {} -> HTTP {} ({})",
            source_name,
            action,
            status.as_u16(),
            status.canonical_reason().unwrap_or("Unknown")
        )
    } else {
        format!("Zabbix [{}]: {} -> {}", source_name, action, err)
    }
}

fn map_zabbix_severity(severity_str: &str) -> String {
    match severity_str {
        "5" => "disaster".to_string(),
        "4" => "high".to_string(),
        "3" => "warning".to_string(),
        "2" => "warning".to_string(),
        "1" => "info".to_string(),
        _ => "info".to_string(),
    }
}

fn format_timestamp(clock_str: &str) -> String {
    if let Ok(secs) = clock_str.parse::<i64>() {
        if secs > 0 {
            if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
                return dt.format("%Y-%m-%d %H:%M:%S").to_string();
            }
        }
    }
    "Unknown".to_string()
}
