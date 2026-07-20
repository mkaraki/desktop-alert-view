# Desktop Alert View 🚨

A sleek, lightweight, desktop widget for monitoring alerts and problem histories across multiple monitoring systems in real-time. Built with **Tauri**, **Rust**, **HTML5**, **CSS3**, and **Vanilla JavaScript**.

> [!NOTE]
> This project made by Antigravity. Not tested well.

## ✨ Features

- **Multi-Instance Support**: Monitor multiple servers simultaneously (e.g. 2 Zabbix servers, 3 Grafana instances, 2 Uptime Kuma endpoints).
- **Desktop Widget Mode**:
  - Borderless, dark glassmorphic design.
  - `skipTaskbar` integration to run seamlessly in the background.
  - Interactive System Tray menu (*Show Widget*, *Open Config Folder*, *Open config.yaml*, *Quit*).
  - Smooth titlebar dragging.
- **Supported Monitoring Providers**:
  - **Zabbix**:
    - Host Group filtering (case-insensitive).
    - Monitored host and trigger status filtering.
    - Trigger dependency suppression (evaluates active dependency triggers across all problems).
    - Excludes suppressed and acknowledged problems.
    - Direct trigger filter jump links (`/zabbix.php?action=problem.view&filter_set=1&filter_show=1&filter_triggerids[]={id}`).
    - Unacknowledged closed problem history tracking.
  - **Grafana**:
    - Organization (`X-Grafana-Org-Id`) and Folder filtering.
    - Supports Alertmanager v2 API, Prometheus API, and Legacy API.
    - Direct alert rule view jump links (`/alerting/grafana/{rule_uid}/view`).
    - Annotations API integration for resolved alert history (`OK` / `Normal` state events).
  - **Uptime Kuma**:
    - Monitor Group and Tag filtering.
    - Basic Auth API key authentication.
    - Prometheus `/metrics` parsing with status page fallback.
- **Unified Feed Layout**:
  - Real-time search filter across alert titles, hostnames, sources, and severities.
  - Stat pill jump buttons for quick smooth scrolling to **On going** and **Resolved** sections.
  - Configurable history limit (`max_resolved_history: 100`).

## 🚀 Getting Started

### Build & Run
To run the app locally:
```bash
cargo tauri dev
```

To build a standalone production release executable:
```bash
cargo tauri build
```

## ⚙️ Configuration (`config.yaml`)

On startup, Desktop Alert View automatically creates a default `config.yaml` in the app's configuration folder if one does not exist.

### Example `config.yaml`

```yaml
refresh_interval_seconds: 15
max_resolved_history: 100

zabbix:
  - name: "Zabbix Prod"
    url: "https://zabbix.example.com"
    api_token: "your_zabbix_api_token"
    host_groups:
      - "Linux Servers"
      - "Database Servers"
    # or host_groups: []

grafana:
  - name: "Grafana Prod"
    url: "https://grafana.example.com"
    api_token: "your_grafana_service_account_token"
    org_id: 1
    folders:
      - "Production"
    # or folders: []

uptime_kuma:
  - name: "Uptime Kuma Main"
    url: "https://uptime.example.com"
    api_token: "your_kuma_api_key"
    monitor_groups:
      - "Websites"
    # or monitor_groups: []
    tags:
      - "critical"
    # or tags: []
```
