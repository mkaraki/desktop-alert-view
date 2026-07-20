// Access Tauri core & window IPC functions
const { invoke } = window.__TAURI__.core;

// State management
let state = {
  activeAlerts: [],
  closedUnackedAlerts: [],
  errors: [],
  searchQuery: '',
  isFetching: false,
  autoRefreshTimer: null,
};

// DOM Elements
const elements = {
  titlebar: document.getElementById('titlebar'),
  liveIndicator: document.getElementById('live-indicator'),
  btnRefresh: document.getElementById('btn-refresh'),
  btnConfig: document.getElementById('btn-config'),
  btnOpenYaml: document.getElementById('btn-open-yaml'),
  btnJumpOngoing: document.getElementById('btn-jump-ongoing'),
  btnJumpResolved: document.getElementById('btn-jump-resolved'),
  searchInput: document.getElementById('search-input'),
  statFiringCount: document.getElementById('stat-firing-count'),
  statUnackedCount: document.getElementById('stat-unacked-count'),
  badgeActiveCount: document.getElementById('badge-active-count'),
  badgeUnackedCount: document.getElementById('badge-unacked-count'),
  activeList: document.getElementById('active-alert-list'),
  unackedList: document.getElementById('unacked-alert-list'),
  errorContainer: document.getElementById('error-container'),
  lastUpdated: document.getElementById('last-updated'),
  contentArea: document.getElementById('content-area'),
  sectionOngoing: document.getElementById('section-ongoing'),
  sectionResolved: document.getElementById('section-resolved'),
};

// Initialize Application
document.addEventListener('DOMContentLoaded', () => {
  setupWindowDragging();
  setupEventListeners();
  fetchAlerts();
  
  // Auto refresh every 15 seconds
  state.autoRefreshTimer = setInterval(fetchAlerts, 15000);
});

// Setup Window Dragging on titlebar elements with instant cursor reset
function setupWindowDragging() {
  const titlebar = elements.titlebar;
  if (!titlebar) return;

  const resetCursor = () => {
    titlebar.style.cursor = '';
  };

  titlebar.addEventListener('mousedown', (e) => {
    if (e.button === 0 && !e.target.closest('button, input, a, .icon-btn, .stat-pill')) {
      titlebar.style.cursor = 'grabbing';
      invoke('start_dragging').catch(() => {});
      // Reset cursor after dragging completes or focus returns
      setTimeout(resetCursor, 200);
    }
  });

  titlebar.addEventListener('mouseup', resetCursor);
  titlebar.addEventListener('mouseleave', resetCursor);
  window.addEventListener('mouseup', resetCursor);
  window.addEventListener('focus', resetCursor);
}

function setupEventListeners() {
  // Manual refresh
  elements.btnRefresh.addEventListener('click', () => {
    fetchAlerts();
  });

  // Open config folder
  elements.btnConfig.addEventListener('click', async () => {
    try {
      await invoke('open_config_folder');
    } catch (err) {
      console.error('Error opening config folder:', err);
    }
  });

  // Open config file directly
  elements.btnOpenYaml.addEventListener('click', async () => {
    try {
      await invoke('open_config_file');
    } catch (err) {
      console.error('Error opening config file:', err);
    }
  });

  // Jump to On going active alerts
  elements.btnJumpOngoing.addEventListener('click', () => {
    elements.sectionOngoing.scrollIntoView({ behavior: 'smooth' });
  });

  // Jump to Closed / Resolved alerts
  elements.btnJumpResolved.addEventListener('click', () => {
    elements.sectionResolved.scrollIntoView({ behavior: 'smooth' });
  });

  // Search input
  elements.searchInput.addEventListener('input', (e) => {
    state.searchQuery = e.target.value.trim().toLowerCase();
    render();
  });
}

// Fetch Alerts via Tauri Rust Backend
async function fetchAlerts() {
  if (state.isFetching) return;
  
  state.isFetching = true;
  updateLiveStatus('fetching');

  try {
    const result = await invoke('fetch_all_alerts');
    state.activeAlerts = result.active_alerts || [];
    state.closedUnackedAlerts = result.closed_unacked_alerts || [];
    state.errors = result.errors || [];
    
    updateLiveStatus(state.errors.length > 0 ? 'error' : 'live');
    const now = new Date();
    elements.lastUpdated.textContent = `Updated ${now.toLocaleTimeString()}`;
  } catch (err) {
    console.error('Failed to fetch alerts:', err);
    state.errors = [String(err)];
    updateLiveStatus('error');
    elements.lastUpdated.textContent = 'Fetch failed';
  } finally {
    state.isFetching = false;
    render();
  }
}

function updateLiveStatus(status) {
  elements.liveIndicator.className = 'pulse-dot ' + status;
}

// Render UI based on state & search query
function render() {
  // Update counter badges
  elements.statFiringCount.textContent = state.activeAlerts.length;
  elements.statUnackedCount.textContent = state.closedUnackedAlerts.length;
  elements.badgeActiveCount.textContent = state.activeAlerts.length;
  elements.badgeUnackedCount.textContent = state.closedUnackedAlerts.length;

  // Render errors if any
  if (state.errors.length > 0) {
    elements.errorContainer.classList.remove('hidden');
    elements.errorContainer.innerHTML = state.errors
      .map((err) => `<div>⚠️ ${escapeHtml(err)}</div>`)
      .join('');
  } else {
    elements.errorContainer.classList.add('hidden');
    elements.errorContainer.innerHTML = '';
  }

  // Filter alerts by search query
  const filteredActive = filterAlerts(state.activeAlerts);
  const filteredUnacked = filterAlerts(state.closedUnackedAlerts);

  // Render Active Section
  renderAlertList(elements.activeList, filteredActive, 'firing');

  // Render Unacknowledged / Resolved Section
  renderAlertList(elements.unackedList, filteredUnacked, 'unacked');
}

function filterAlerts(alerts) {
  if (!state.searchQuery) return alerts;
  return alerts.filter(
    (a) =>
      a.title.toLowerCase().includes(state.searchQuery) ||
      a.hostname.toLowerCase().includes(state.searchQuery) ||
      a.source_name.toLowerCase().includes(state.searchQuery) ||
      a.severity.toLowerCase().includes(state.searchQuery)
  );
}

function renderAlertList(container, alerts, type) {
  if (alerts.length === 0) {
    if (type === 'firing') {
      container.innerHTML = `
        <div class="empty-state">
          <div class="empty-icon">✅</div>
          <div class="empty-title">All Systems Normal</div>
          <div class="empty-desc">${
            state.searchQuery
              ? 'No active alerts matching search criteria.'
              : 'No active problems or alerts firing across configured instances.'
          }</div>
        </div>
      `;
    } else {
      container.innerHTML = `
        <div class="empty-state">
          <div class="empty-icon">🎉</div>
          <div class="empty-title">No Closed / Resolved Alerts</div>
          <div class="empty-desc">${
            state.searchQuery
              ? 'No resolved alerts matching search criteria.'
              : 'No recently resolved alerts recorded.'
          }</div>
        </div>
      `;
    }
    return;
  }

  container.innerHTML = alerts
    .map((item) => createAlertCardHtml(item))
    .join('');

  // Attach click events for direct jump links
  container.querySelectorAll('.alert-card').forEach((card) => {
    card.addEventListener('click', async (e) => {
      const url = card.getAttribute('data-jump-url');
      if (url) {
        try {
          await invoke('open_external_url', { url });
        } catch (err) {
          console.error('Failed to open jump URL:', err);
        }
      }
    });
  });
}

function createAlertCardHtml(item) {
  const sevClass = `sev-${item.severity.toLowerCase()}`;
  const sourceTypeClass = `type-${item.source_type.toLowerCase()}`;
  const severityLabel = getSeverityLabel(item.severity);

  return `
    <div class="alert-card ${sevClass}" data-jump-url="${escapeHtml(item.jump_url)}">
      <div class="card-header">
        <div class="card-tags">
          <span class="source-tag ${sourceTypeClass}">${escapeHtml(item.source_name)}</span>
          <span class="severity-badge">${severityLabel}</span>
        </div>
        <span class="card-time">${escapeHtml(item.timestamp)}</span>
      </div>
      <div class="card-title">${escapeHtml(item.title)}</div>
      <div class="card-footer">
        <div class="host-pill" title="${escapeHtml(item.hostname)}">
          💻 ${escapeHtml(item.hostname)}
        </div>
        <div class="jump-action">
          Jump ↗
        </div>
      </div>
    </div>
  `;
}

function getSeverityLabel(sev) {
  switch (sev.toLowerCase()) {
    case 'disaster': return 'DISASTER';
    case 'down': return 'DOWN';
    case 'high': return 'HIGH';
    case 'warning': return 'WARNING';
    case 'info': return 'INFO';
    case 'resolved_unacked': return 'RESOLVED';
    default: return sev.toUpperCase();
  }
}

function escapeHtml(str) {
  if (!str) return '';
  return String(str)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#039;');
}
