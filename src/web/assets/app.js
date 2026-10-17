'use strict';

var TICK_MS = 1000;
var POLL_INITIAL_MS = 2000;
var POLL_MAX_MS = 30000;
var MAX_SSE_FAILURES = 3;
var JOB_PREVIEW_LEN = 120;
var TOKEN_KEY = 'dmuxai.token';
var STATUS_ORDER = [
  'pending', 'ready', 'running', 'waiting_gate', 'completed', 'failed', 'skipped'
];

function storageGet(key) {
  try { return window.sessionStorage.getItem(key); } catch (err) { return null; }
}

function storageSet(key, value) {
  try { window.sessionStorage.setItem(key, value); } catch (err) { /* ignore */ }
}

function storageRemove(key) {
  try { window.sessionStorage.removeItem(key) ; } catch (err) { /* ignore */ }
}

// ---------------------------------------------------------------------------
// Theme and accent (SPEC-theme-colors FR-603, UX-600, NFR-600).
// Choices persist only in dmuxai.theme / dmuxai.accent localStorage keys
// (never the token); styles are applied via element style properties only,
// which the CSP allows.
// ---------------------------------------------------------------------------

var THEME_KEY = 'dmuxai.theme';
var ACCENT_KEY = 'dmuxai.accent';
var THEME_VALUES = ['light', 'dark', 'system'];
var themePref = null;
// FR-603: the snapshot ui.theme/ui.accent only apply until the user makes an
// in-session choice. The flags are in-memory so also work when localStorage
// is unavailable; a stored key counts as an already-made choice.
var userThemeChosen = false;
var userAccentChosen = false;
// Latest valid snapshot values (from [web] config); the reset button falls
// back to the snapshot accent once.
var snapshotUiTheme = null;
var snapshotAccent = null;

var colorSchemeMedia = typeof window.matchMedia === 'function'
  ? window.matchMedia('(prefers-color-scheme: dark)')
  : null;

var colorSchemeListener = function () {
  if (themePref === 'system') { applyResolvedTheme(); }
};

function isHexColor(value) {
  return typeof value === 'string' && /^#[0-9a-fA-F]{6}$/.test(value);
}

// WCAG 2.1 relative luminance of an #rrggbb color.
function relativeLuminance(value) {
  var channels = [
    parseInt(value.slice(1, 3), 16),
    parseInt(value.slice(3, 5), 16),
    parseInt(value.slice(5, 7), 16)
  ];
  var total = 0;
  var weights = [0.2126, 0.7152, 0.0722];
  for (var i = 0; i < channels.length; i++) {
    var c = channels[i] / 255;
    var linear = c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
    total += weights[i] * linear;
  }
  return total;
}

function contrastRatio(a, b) {
  var la = relativeLuminance(a);
  var lb = relativeLuminance(b);
  var hi = Math.max(la, lb);
  var lo = Math.min(la, lb);
  return (hi + 0.05) / (lo + 0.05);
}

// NFR-600: text drawn on the accent uses black or white, whichever has the
// higher WCAG contrast against the accent.
function onAccentFor(value) {
  var luminance = relativeLuminance(value);
  var onBlack = (luminance + 0.05) / 0.05;        // black text on the accent
  var onWhite = 1.05 / (luminance + 0.05);        // white text on the accent
  return onBlack >= onWhite ? '#000000' : '#ffffff';
}

function validTheme(value) {
  return THEME_VALUES.indexOf(value) !== -1;
}

function applyResolvedTheme() {
  var resolved = themePref;
  if (resolved === 'system') {
    resolved = colorSchemeMedia && colorSchemeMedia.matches ? 'dark' : 'light';
  }
  if (resolved !== 'dark' && resolved !== 'light') { resolved = 'dark'; }
  document.documentElement.dataset.theme = resolved;
}

function applyAccentOverride(value) {
  var style = document.documentElement.style;
  if (!isHexColor(value)) {
    style.removeProperty('--accent');
    style.removeProperty('--on-accent');
    return;
  }
  var hex = value.toLowerCase();
  style.setProperty('--accent', hex);
  style.setProperty('--on-accent', onAccentFor(hex));
}

function syncAccentPicker() {
  var picker = $('accent-picker');
  if (!picker) { return; }
  var current = window.getComputedStyle(document.documentElement)
    .getPropertyValue('--accent').trim();
  if (isHexColor(current)) { picker.value = current.toLowerCase(); }
}

// User picked a theme in the header; persist when the browser allows it.
function setThemeChoice(pref) {
  themePref = validTheme(pref) ? pref : 'system';
  userThemeChosen = true;
  try { window.localStorage.setItem(THEME_KEY, themePref); } catch (err) { /* ignore */ }
  applyResolvedTheme();
  setThemeSelect();
}

function setThemeSelect() {
  var select = $('theme-select');
  if (select) { select.value = validTheme(themePref) ? themePref : 'system'; }
}

// User picked an accent in the header; persist when the browser allows it.
function setAccentChoice(value) {
  var valid = isHexColor(value);
  userAccentChosen = true;
  try {
    if (valid) { window.localStorage.setItem(ACCENT_KEY, value.toLowerCase()); }
    else { window.localStorage.removeItem(ACCENT_KEY); }
  } catch (err) { /* ignore */ }
  applyAccentOverride(valid ? value : null);
  syncAccentPicker();
}

// Reset: drop the stored accent once and fall back to the configured (snapshot)
// accent immediately, so the palette default is never painted in between.
function resetAccent() {
  userAccentChosen = true;
  try { window.localStorage.removeItem(ACCENT_KEY); } catch (err) { /* ignore */ }
  applyAccentOverride(snapshotAccent);
  syncAccentPicker();
}

function storedTheme() {
  try { return window.localStorage.getItem(THEME_KEY); } catch (err) { return null; }
}

function storedAccent() {
  try { return window.localStorage.getItem(ACCENT_KEY); } catch (err) { return null; }
}

function bindThemeControls() {
  var select = $('theme-select');
  var picker = $('accent-picker');
  var reset = $('accent-reset');
  if (!select || !picker || !reset) { return; }
  select.addEventListener('change', function() {
    setThemeChoice(select.value);
  });
  picker.addEventListener('input', function() {
    setAccentChoice(picker.value);
  });
  reset.addEventListener('click', resetAccent);
}

// FR-603: while the user has made no in-session choice, the page follows the
// snapshot ui.theme / ui.accent (from [web] config); missing or invalid values
// are ignored. Once the user picks theme or accent, or hits Reset, the flags
// above stop further snapshot applications.
function applySnapshotUi(snap) {
  var ui = snap && snap.ui ? snap.ui : null;
  if (ui && validTheme(String(ui.theme || ''))) {
    snapshotUiTheme = String(ui.theme);
  }
  if (ui && isHexColor(ui.accent)) {
    snapshotAccent = String(ui.accent).toLowerCase();
  }
  if (!userThemeChosen && snapshotUiTheme) {
    themePref = snapshotUiTheme;
    applyResolvedTheme();
    setThemeSelect();
  }
  if (!userAccentChosen) {
    applyAccentOverride(snapshotAccent);
    syncAccentPicker();
  }
}

// Use the ?token= from the URL when present, stash it for refreshes, and hide
// it from the address bar afterwards (deepseek cross-review finding 3).
var token = (function () {
  var fromQuery = new URLSearchParams(location.search).get('token');
  if (fromQuery) {
    storageSet(TOKEN_KEY, fromQuery);
    history.replaceState(null, '', location.pathname);
    return fromQuery;
  }
  return storageGet(TOKEN_KEY);
})();

var snapshot = null;
var sse = null;
var sseFailures = 0;
var pollTimer = null;
var pollDelay = POLL_INITIAL_MS;
var tickTimer = null;
var lastRendered = null;

function $(id) { return document.getElementById(id); }

function showMsg(text, isError) {
  var el = $('msg');
  el.hidden = false;
  el.textContent = text;
  el.classList.toggle('notice-error', Boolean(isError));
}

function hideMsg() {
  var el = $('msg');
  el.hidden = true;
  el.textContent = '';
}

function setConn(state, label) {
  var el = $('conn');
  el.textContent = label;
  el.classList.remove('conn-connecting', 'conn-live', 'conn-poll', 'conn-error');
  el.classList.add('conn-' + state);
}

function fmtElapsed(ms) {
  if (typeof ms !== 'number' || ms < 0 || !isFinite(ms)) {
    return null;
  }
  var s = Math.floor(ms / 1000);
  var h = Math.floor(s / 3600);
  var m = Math.floor((s % 3600) / 60);
  s = s % 60;
  var out = '';
  if (h > 0) { out += h + 'h'; }
  if (h > 0 || m > 0) { out += m + 'm'; }
  out += s + 's';
  return out;
}

function truncate(text, max) {
  if (typeof text !== 'string') { return ''; }
  if (text.length <= max) { return text; }
  return text.slice(0, max) + '\u2026';
}

function el(tag, className, text) {
  var node = document.createElement(tag);
  if (className) { node.className = className; }
  if (text !== undefined && text !== null) { node.textContent = text; }
  return node;
}

function badge(status) {
  var label = String(status || 'pending');
  return el('span', 'badge badge-' + label, label);
}

function graphCounts(graph) {
  var counts = graph && graph.counts ? graph.counts : null;
  var raw = counts || {};
  var out = [];
  for (var i = 0; i < STATUS_ORDER.length; i++) {
    var key = STATUS_ORDER[i];
    out.push({
      key: key,
      n: typeof raw[key] === 'number' && raw[key] > 0 ? raw[key] : 0
    });
  }
  return out;
}

function barSummary(items, total) {
  if (total === 0) {
    return 'No nodes in the current graph run';
  }
  var parts = [];
  for (var i = 0; i < items.length; i++) {
    if (items[i].n > 0) {
      parts.push(items[i].n + ' ' + items[i].key.replace('_', ' '));
    }
  }
  return 'Graph progress: ' + parts.join(', ') + ', ' + total + ' total';
}

function renderBar(graph) {
  var bar = $('graph-bar');
  bar.replaceChildren();
  var items = graphCounts(graph);
  var total = 0;
  for (var i = 0; i < items.length; i++) { total += items[i].n; }
  bar.setAttribute('role', 'img');
  bar.setAttribute('aria-label', barSummary(items, total));
  if (total === 0) {
    bar.classList.add('bar-empty');
    return;
  }
  bar.classList.remove('bar-empty');
  for (var j = 0; j < items.length; j++) {
    var seg = items[j];
    if (seg.n <= 0) { continue; }
    var piece = el('span', 'bar-seg bar-status-' + seg.key);
    piece.style.width = Math.round((seg.n / total) * 1000) / 10 + '%';
    piece.title = seg.key + ': ' + seg.n;
    bar.appendChild(piece);
  }
}

function renderCounts(graph) {
  var target = $('graph-counts');
  var items = graphCounts(graph);
  var parts = [];
  for (var i = 0; i < items.length; i++) {
    if (items[i].n <= 0) { continue; }
    parts.push(items[i].n + ' ' + items[i].key);
  }
  target.replaceChildren();
  target.appendChild(document.createTextNode(parts.length > 0 ? parts.join(' \u00b7 ') : 'no nodes yet'));
}

function nodeElapsed(node, snap, nowMs) {
  if (node.status === 'running') {
    if (
      typeof node.started_at_ms === 'number' &&
      typeof nowMs === 'number' &&
      snap &&
      typeof snap.generated_at_ms === 'number' &&
      typeof snap.receivedAt === 'number'
    ) {
      // Server and browser clocks may disagree; interpolate from the
      // server send time (sent_at_ms) plus the time our page held the snapshot.
      var serverNow = snap.sent_at_ms > 0 ? snap.sent_at_ms : snap.generated_at_ms;
      return Math.max(
        0,
        (serverNow - node.started_at_ms) + (nowMs - snap.receivedAt)
      );
    }
    return null;
  }
  if (typeof node.elapsed_ms === 'number') {
    return node.elapsed_ms;
  }
  if (node.status === 'pending' || node.status === 'ready' || node.status === 'waiting_gate') {
    return null;
  }
  if (node.status === 'completed' || node.status === 'failed' || node.status === 'skipped') {
    if (typeof node.finished_at_ms === 'number' && typeof node.started_at_ms === 'number') {
      return node.finished_at_ms - node.started_at_ms;
    }
    return null;
  }
  return null;
}

function renderNodeTime(head, node, snap, nowMs) {
  var cell = el('span', 'node-time');
  var ms = nodeElapsed(node, snap, nowMs);
  if (ms !== null) {
    cell.textContent = fmtElapsed(ms);
  }
  head.appendChild(cell);
}

function renderNodes(graph, nowMs) {
  var list = $('nodes');
  list.replaceChildren();
  var nodes = graph && Array.isArray(graph.nodes) ? graph.nodes : [];
  var current = graph && Array.isArray(graph.current_nodes) ? graph.current_nodes : [];

  if (nodes.length === 0) {
    list.appendChild(el('p', 'empty', 'no nodes in this run'));
    return;
  }

  for (var i = 0; i < nodes.length; i++) {
    var node = nodes[i];
    var item = el('li', 'node');
    if (current.indexOf(node.id) !== -1) {
      item.classList.add('node-current');
      item.setAttribute('aria-current', 'step');
    }

    var head = el('div', 'node-head');
    var title = el('div', 'node-title');
    title.appendChild(badge(node.status));
    var name = el('span', 'node-name', node.id);
    name.title = node.id;
    title.appendChild(name);
    if (typeof node.attempt === 'number' && node.attempt > 0) {
      title.appendChild(el('span', 'node-attempt', 'attempt ' + node.attempt));
    }
    head.appendChild(title);
    renderNodeTime(head, node, snapshot, nowMs);
    item.appendChild(head);

    var meta = el('div', 'node-meta');
    meta.appendChild(el('span', 'node-role', String(node.role || '')));
    meta.appendChild(el('span', 'dot', '\u00b7'));
    meta.appendChild(el('span', 'node-runner', String(node.runner || '')));
    if (node.model_id) {
      meta.appendChild(el('span', 'dot', '\u00b7'));
      meta.appendChild(el('span', 'node-model', String(node.model_id || '')));
    }
    item.appendChild(meta);

    var job = el('p', 'node-job', truncate(node.job, JOB_PREVIEW_LEN));
    job.title = node.job || '';
    item.appendChild(job);

    list.appendChild(item);
  }
}

function renderGateHint(graph) {
  var hint = $('gate-hint');
  if (!graph || !graph.waiting_gate_node) {
    hint.hidden = true;
    hint.textContent = '';
    return;
  }
  hint.hidden = false;
  hint.textContent =
    'Waiting for gate at "' + graph.waiting_gate_node +
    '". Type /graph continue in the TUI to proceed.';
}

function renderAgents(agents) {
  var wrap = $('agents');
  wrap.replaceChildren();
  var list = Array.isArray(agents) ? agents : [];
  if (list.length === 0) {
    wrap.appendChild(el('p', 'empty', 'no agents'));
    return;
  }
  for (var i = 0; i < list.length; i++) {
    var agent = list[i] || {};
    var chip = el('span', 'chip');
    // FR-604: the snapshot carries the TUI agent color (#rrggbb or absent);
    // use it as a colored dot on the chip when present.
    if (isHexColor(agent.color)) {
      var dot = el('span', 'chip-dot');
      dot.style.backgroundColor = String(agent.color).toLowerCase();
      chip.appendChild(dot);
    }
    var name = el('span', 'chip-name', String(agent.label || agent.id || ''));
    chip.appendChild(name);

    var bits = [];
    if (agent.busy) {
      chip.classList.add('chip-busy');
      bits.push('busy');
    }
    if (agent.phase) { bits.push(String(agent.phase)); }
    if (agent.model) { bits.push(String(agent.model)); }
    if (bits.length > 0) {
      var detail = bits.join(' \u00b7 ');
      chip.appendChild(el('span', 'chip-meta', detail));
    }
    wrap.appendChild(chip);
  }
}

// ---------------------------------------------------------------------------
// To-dos (SPEC-web-dashboard FR-541 / FR-542 / UX-540, TC-543).
// ---------------------------------------------------------------------------

var TODO_TITLE_MAX = 200;
// Transient per-item errors from our own requests, and the id of the item
// currently being inline-edited. Both survive snapshot re-renders: the item
// under edit is never destroyed, the other items are re-rendered (UX-540).
var todoItemErrors = {};
var todoEditId = null;

function todoUrl(id) {
  return '/api/todos/' + encodeURIComponent(id);
}

function todoApi(method, path, body) {
  var headers = { Authorization: 'Bearer ' + token };
  var init = { method: method, headers: headers, cache: 'no-store' };
  if (body !== undefined) {
    headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  return fetch(path, init).then(function(res) {
    if (res.status === 401) {
      var fatal = new Error('unauthorized');
      fatal.fatal = true;
      throw fatal;
    }
    return res.text().then(function(text) {
      var payload = null;
      if (text) {
        try {
          payload = JSON.parse(text);
        } catch (err) {
          payload = null;
        }
      }
      if (!res.ok) {
        var message = payload && typeof payload.error === 'string'
          ? payload.error
          : 'HTTP ' + res.status;
        var hint = message.indexOf('dmuxai --web') !== -1;
        if (hint) {
          message = message + ' To-do runs need "dmuxai --web" ' +
            '(open the dashboard URL printed by "dmuxai --web").';
        }
        var failure = new Error(message);
        failure.status = res.status;
        failure.webModeHint = hint;
        throw failure;
      }
      return payload;
    });
  });
}

function refreshTodos() {
  // One immediate state fetch so a mutation shows up even without SSE;
  // regular snapshot pushes keep updating the list afterwards.
  poll();
}

function jiraPreviewParts(rawValue) {
  var value = String(rawValue || '').trim();
  if (!value) { return null; }
  var key = null;
  var url = null;
  if (TODO_KEY_RE.test(value)) {
    key = value;
    var base = String((snapshot || {}).jira_base_url || '').replace(new RegExp(/\/+$/), '');
    if (TODO_URL_RE.test(base)) {
      url = base + '/browse/' + encodeURIComponent(key);
    }
  } else if (TODO_URL_RE.test(value)) {
    url = value;
    var m = value.match(new RegExp(/\/browse\/([A-Za-z][A-Za-z0-9_]*-\d+)/));
    if (m) { key = m[1]; }
  } else {
    return { error: 'Jira ticket: expected KEY-123 or http(s) URL.' };
  }
  var existing = key ? ticketByExternalKey(key) : null;
  return { key: key, url: url, existing: existing };
}

function updateJiraPreview() {
  var preview = $('todo-jira-preview');
  var input = $('todo-jira');
  if (!preview || !input) { return; }
  var parts = jiraPreviewParts(input.value);
  preview.replaceChildren();
  if (!parts) {
    preview.hidden = true;
    return;
  }
  preview.hidden = false;
  if (parts.error) {
    preview.classList.add('todo-jira-preview-error');
    preview.textContent = parts.error;
    return;
  }
  preview.classList.remove('todo-jira-preview-error');
  var resolved = el('span', 'todo-jira-key', parts.key || '');
  if (parts.url) {
    var link = jiraTicketLink(parts.url);
    link.textContent = parts.key || parts.url;
    resolved.replaceChildren(link);
    preview.title = parts.url;
  } else {
    preview.title = 'No [todo] jira_base_url configured; the key is stored without a link.';
  }
  preview.appendChild(resolved);
  if (parts.existing) {
    var chip = el('span', jiraTicketClass(parts.existing.external_status), '');
    chip.appendChild(document.createTextNode(
      String(parts.existing.external_status || '?') + ' \u00b7 ' + String(parts.existing.title || '')
    ));
    preview.appendChild(document.createTextNode(' \u2014 existing: '));
    preview.appendChild(chip);
  }
}

function submitNewTodo() {
  var titleInput = $('todo-title');
  var bodyInput = $('todo-body');
  var title = titleInput.value.trim();
  if (!title) {
    showMsg('To-do title is required.', true);
    titleInput.focus();
    return;
  }
  var body = { title: title, body: bodyInput.value };
  var parts = jiraPreviewParts($('todo-jira').value);
  if (parts && parts.error) {
    showMsg(parts.error, true);
    $('todo-jira').focus();
    return;
  }
  if (parts) {
    if (parts.key) { body.external_key = parts.key; }
    if (parts.url) { body.link_url = parts.url; }
  }
  todoApi('POST', '/api/todos', body)
    .then(function() {
      titleInput.value = '';
      bodyInput.value = '';
      $('todo-jira').value = '';
      updateJiraPreview();
      titleInput.focus();
      refreshTodos();
    })
    .catch(function(err) {
      if (err && err.fatal) {
        onUnauthorized();
        return;
      }
      showMsg('Could not add to-do: ' + err.message, true);
    });
}

function setTodoItemError(id, message) {
  todoItemErrors[id] = { message: message };
  renderTodos();
}

function todoErrorRow(id) {
  var row = el('p', 'todo-error');
  var err = todoItemErrors[id];
  if (err) {
    row.hidden = false;
    row.textContent = err.message;
  }
  return row;
}

function refreshItemErrorRow(container, id) {
  var row = container.querySelector('.todo-error');
  if (!row) { return; }
  var err = todoItemErrors[id];
  row.hidden = !err;
  row.textContent = err ? err.message : '';
}

function patchTodo(id, patch, clearEdit) {
  todoApi('PATCH', todoUrl(id), patch)
    .then(function() {
      delete todoItemErrors[id];
      if (clearEdit && todoEditId === id) { todoEditId = null; }
      refreshTodos();
    })
    .catch(function(err) {
      if (err && err.fatal) {
        onUnauthorized();
        return;
      }
      setTodoItemError(id, err.message);
    });
}

function runTodo(id, body) {
  todoApi('POST', todoUrl(id) + '/run', body)
    .then(function() {
      delete todoItemErrors[id];
      refreshTodos();
    })
    .catch(function(err) {
      if (err && err.fatal) {
        onUnauthorized();
        return;
      }
      setTodoItemError(id, err.message);
    });
}

function deleteTodo(id) {
  todoApi('DELETE', todoUrl(id))
    .then(function() {
      delete todoItemErrors[id];
      if (todoEditId === id) { todoEditId = null; }
      refreshTodos();
    })
    .catch(function(err) {
      if (err && err.fatal) {
        onUnauthorized();
        return;
      }
      setTodoItemError(id, err.message);
    });
}

function startEdit(id) {
  todoEditId = id;
  renderTodos();
}

function cancelEdit() {
  delete todoItemErrors[todoEditId];
  todoEditId = null;
  renderTodos();
}

function todoButton(className, label, handler) {
  var btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'btn ' + className;
  btn.textContent = label;
  btn.addEventListener('click', handler);
  return btn;
}

function todoDoneCheckbox(id, status) {
  var box = document.createElement('input');
  box.type = 'checkbox';
  box.className = 'todo-done';
  box.checked = status === 'done';
  var label = status === 'done' ? 'Mark to-do open' : 'Mark to-do done';
  box.setAttribute('aria-label', label);
  box.title = label;
  box.addEventListener('change', function() {
    patchTodo(id, { status: box.checked ? 'done' : 'open' }, false);
  });
  return box;
}

function todoSourceBadge(todo) {
  var source = String(todo.source || 'manual');
  if (source === 'manual') {
    return null;
  }
  var wrap = el('span', 'todo-source-wrap');
  wrap.appendChild(el('span', 'badge badge-src', source));
  if (todo.source_ref) {
    var ref = el('span', 'todo-source-ref', String(todo.source_ref));
    ref.title = String(todo.source_ref);
    wrap.appendChild(ref);
  }
  return wrap;
}

// ---------------------------------------------------------------------------
// Jira ticket strip (SPEC-web-dashboard UX-561, TC-572). A thin Jira-blue
// strip above the card body shows the resolved key/link, status chip by
// category, assignee, source tag and link host; cards without a key/link get
// a dashed "+ Jira" affordance for inline key/URL editing.
// ---------------------------------------------------------------------------

var TODO_URL_RE = new RegExp(/^https?:\/\/.+/i);
var TODO_KEY_RE = new RegExp(/^[A-Za-z][A-Za-z0-9_]*-\d+$/);

function ticketByExternalKey(key) {
  var snap = snapshot || null;
  var todos = snap && Array.isArray(snap.todos) ? snap.todos : [];
  for (var i = 0; i < todos.length; i++) {
    var other = todos[i] || {};
    if (String(other.external_key || '') === key) { return other; }
  }
  return null;
}

function jiraTicketClass(status) {
  var s = String(status || '');
  if (s === 'To Do' || s === 'Open' || s === 'Backlog' || s === 'Selected for Development') {
    return 'todo-jira-chip todo-jira-chip-todo';
  }
  if (s === 'In Progress' || s === 'In Review' || s === 'Reopened') {
    return 'todo-jira-chip todo-jira-chip-progress';
  }
  if (s === 'Done' || s === 'Closed' || s === 'Resolved') {
    return 'todo-jira-chip todo-jira-chip-done';
  }
  return 'todo-jira-chip todo-jira-chip-neutral';
}

function jiraTicketLink(href) {
  var a = document.createElement('a');
  a.className = 'todo-jira-link';
  a.href = href;
  a.target = '_blank';
  a.rel = 'noopener noreferrer';
  return a;
}

function todoStrip(todo) {
  var key = String(todo.external_key || '');
  var url = String(todo.link_url || '');
  var strip = el('div', 'todo-jira-strip');
  var parts = [];
  var i;

  if (key) {
    var linked = url && TODO_URL_RE.test(url);
    var keyNode = linked ? jiraTicketLink(url) : el('span', 'todo-jira-key');
    keyNode.classList.add('todo-jira-key');
    keyNode.textContent = key;
    if (linked) { keyNode.title = url; }
    parts.push(keyNode);
  }

  var status = String(todo.external_status || '').trim();
  if (status) {
    parts.push(el('span', jiraTicketClass(status), status));
  }

  var assignee = String(todo.external_assignee || '').trim();
  if (assignee) {
    parts.push(el('span', 'todo-jira-assignee', assignee));
  } else {
    parts.push(el('span', 'todo-jira-assignee todo-jira-unassigned', 'Unassigned'));
  }

  if (todo.source === 'jira') {
    parts.push(el('span', 'todo-jira-tag todo-jira-tag-jira', 'imported'));
  } else {
    parts.push(el('span', 'todo-jira-tag', 'manual'));
  }

  for (i = 0; i < parts.length; i++) {
    if (i > 0) {
      strip.appendChild(el('span', 'todo-jira-sep', '\u00b7'));
    }
    strip.appendChild(parts[i]);
  }

  if (url && TODO_URL_RE.test(url)) {
    var host = '';
    try { host = new URL(url).host || ''; } catch (err) { host = ''; }
    if (host) {
      strip.appendChild(el('span', 'todo-jira-host', host + ' \u2197'));
    }
  }

  return strip;
}

function todoLinker(id) {
  var wrap = el('div', 'todo-jira-linker');
  var trigger = todoButton('btn btn-jira', '+ Jira', function() {
    trigger.hidden = true;
    form.hidden = false;
    input.focus();
  });
  var form = el('span', 'todo-jira-form');
  form.hidden = true;
  var input = document.createElement('input');
  input.type = 'text';
  input.className = 'todo-jira-edit-input';
  input.maxLength = 2048;
  input.placeholder = 'KEY-123 or https://jira.example.com/browse/KEY-123';
  input.setAttribute('aria-label', 'Jira ticket key or URL');
  var apply = todoButton('btn btn-primary btn-jira-apply', 'Apply', addJira);
  var cancel = todoButton('btn btn-jira-cancel', 'Cancel', unboltEdit);

  function addJira() {
    var value = input.value.trim();
    if (!value) {
      setTodoItemError(id, 'Jira ticket: enter a KEY-N or URL.');
      return;
    }
    if (TODO_KEY_RE.test(value)) {
      patchTodo(id, { external_key: value }, false);
      return;
    }
    if (TODO_URL_RE.test(value)) {
      patchTodo(id, { link_url: value }, false);
      return;
    }
    setTodoItemError(id, 'Jira ticket: expected KEY-123 or http(s) URL.');
  }

  function unboltEdit() {
    delete todoItemErrors[id];
    input.value = '';
    form.hidden = true;
    trigger.hidden = false;
    renderTodos();
  }

  form.appendChild(input);
  form.appendChild(apply);
  form.appendChild(cancel);
  wrap.appendChild(trigger);
  wrap.appendChild(form);

  input.addEventListener('keydown', function(e) {
    if (e.key === 'Enter') { addJira(); }
    if (e.key === 'Escape') { unboltEdit(); }
  });
  return wrap;
}

function todoRunInfoRow(todo) {
  var kind = todo.run_kind ? String(todo.run_kind) : '';
  var ref = todo.run_ref ? String(todo.run_ref) : '';
  var row = el('p', 'todo-runinfo');
  row.textContent = 'run: ' + (kind || 'unknown') + (ref ? ' \u00b7 ' + ref : '');
  if (ref) { row.title = ref; }
  return row;
}

function todoViewActions(id, todo) {
  var wrap = el('span', 'todo-actions');
  wrap.appendChild(todoButton('btn-edit', 'Edit', function() {
    startEdit(id);
  }));
  wrap.appendChild(todoButton('btn-delete btn-danger', 'Delete', function() {
    if (window.confirm('Delete to-do "' + String(todo.title || '') + '"?')) {
      deleteTodo(id);
    }
  }));
  return wrap;
}

function todoRunControls(id, status) {
  var row = el('div', 'todo-controls');
  var locked = status === 'running' || status === 'done';

  var snap = snapshot || {};
  // OQ-521: serve mode publishes source "files"; runs are refused there with
  // 409, so the controls are disabled up front (the inline 409 stays as a
  // fallback for a stale server mode). Add/edit/done/delete stay enabled.
  var viewOnly = String(snap.source || '') === 'files';
  var viewTitle = 'View-only dashboard: start "dmuxai --web" to run to-dos from the browser';

  var agents = Array.isArray(snap.agents) ? snap.agents : [];

  var select = el('select', 'todo-agent-select');
  select.setAttribute('aria-label', 'Agent to run this to-do');
  for (var i = 0; i < agents.length; i++) {
    var agent = agents[i] || {};
    var opt = document.createElement('option');
    opt.value = String(agent.id || '');
    opt.textContent = String(agent.label || agent.id || '');
    if (agent.busy) { opt.disabled = true; }
    select.appendChild(opt);
  }
  row.appendChild(select);

  var runBtn = todoButton('btn-run', 'Run', function() {
    var chosen = select.value;
    var active = select.selectedIndex >= 0
      ? select.options[select.selectedIndex]
      : null;
    if (!chosen || (active && active.disabled)) { return; }
    runTodo(id, { mode: 'agent', agent: chosen });
  });
  var graphBtn = todoButton('btn-graph', 'Run as graph', function() {
    runTodo(id, { mode: 'graph' });
  });

  function syncRunDisabled() {
    var active = select.selectedIndex >= 0
      ? select.options[select.selectedIndex]
      : null;
    var runDisabled = locked || viewOnly || !select.value || Boolean(active && active.disabled);
    var graphDisabled = locked || viewOnly;
    runBtn.disabled = runDisabled;
    runBtn.setAttribute('aria-disabled', runDisabled ? 'true' : 'false');
    graphBtn.disabled = graphDisabled;
    graphBtn.setAttribute('aria-disabled', graphDisabled ? 'true' : 'false');
    select.setAttribute('aria-disabled', viewOnly ? 'true' : 'false');
    if (viewOnly) {
      row.title = viewTitle;
      select.title = viewTitle;
      runBtn.title = viewTitle;
      graphBtn.title = viewTitle;
    }
  }
  select.addEventListener('change', syncRunDisabled);
  syncRunDisabled();

  row.appendChild(runBtn);
  row.appendChild(graphBtn);
  return row;
}

function buildTodoEditor(item, todo) {
  var id = String(todo.id || '');

  var titleField = el('label', 'todo-field todo-field-title');
  titleField.appendChild(el('span', 'field-label', 'Title'));
  var titleInput = document.createElement('input');
  titleInput.type = 'text';
  titleInput.className = 'todo-edit-title';
  titleInput.maxLength = TODO_TITLE_MAX;
  titleInput.value = String(todo.title || '');
  titleField.appendChild(titleInput);
  item.appendChild(titleField);

  var bodyField = el('label', 'todo-field');
  bodyField.appendChild(el('span', 'field-label', 'Body'));
  var bodyInput = document.createElement('textarea');
  bodyInput.className = 'todo-edit-body';
  bodyInput.rows = 3;
  bodyInput.value = String(todo.body || '');
  bodyField.appendChild(bodyInput);
  item.appendChild(bodyField);

  var actions = el('div', 'todo-actions');
  actions.appendChild(todoButton('btn-save', 'Save', function() {
    var title = titleInput.value.trim();
    if (!title) {
      setTodoItemError(id, 'Title is required.');
      titleInput.focus();
      return;
    }
    patchTodo(id, { title: title, body: bodyInput.value }, true);
  }));
  actions.appendChild(todoButton('btn-cancel', 'Cancel', function() {
    cancelEdit();
  }));
  item.appendChild(actions);

  item.appendChild(todoErrorRow(id));

  titleInput.focus();
}

function todoNearBrowseable(viewOnly) {
  return String((snapshot || {}).source || '') === 'files' ? true : viewOnly;
}

function buildTodoView(item, todo) {
  var id = String(todo.id || '');
  var status = String(todo.status || 'open');

  // UX-561: key/link cards get the strip; cards without one offer "+ Jira"
  // for inline key/URL editing (allowed in serve mode too: it edits the
  // to-do, it does not run anything).
  var hasStrip = Boolean(todo.external_key || todo.link_url);
  if (hasStrip) {
    item.classList.add('has-jira');
    item.appendChild(todoStrip(todo));
  } else {
    item.appendChild(todoLinker(id));
  }

  var head = el('div', 'todo-head');
  head.appendChild(todoDoneCheckbox(id, status));
  head.appendChild(badge(status));
  // The strip already shows key/link + source tag; without it the legacy
  // source badge/source-ref key would duplicate the strip contents.
  if (!hasStrip) {
    var sourceBadge = todoSourceBadge(todo);
    if (sourceBadge) { head.appendChild(sourceBadge); }
  }
  var title = el('span', 'todo-title', String(todo.title || ''));
  title.title = String(todo.title || '');
  head.appendChild(title);
  head.appendChild(todoViewActions(id, todo));
  item.appendChild(head);

  if (status === 'running') {
    item.appendChild(todoRunInfoRow(todo));
  }
  if (todo.last_error) {
    item.appendChild(el('p', 'todo-last-error', String(todo.last_error)));
  }
  item.appendChild(todoErrorRow(id));
  item.appendChild(todoRunControls(id, status));
}

function renderTodoItem(todo) {
  var id = String(todo.id || '');
  var status = String(todo.status || 'open');
  var item = el('article', 'todo todo-status-' + status);
  item.setAttribute('data-todo-id', id);

  if (todoEditId !== null && id === todoEditId) {
    item.classList.add('todo-editing');
    buildTodoEditor(item, todo);
    return item;
  }
  buildTodoView(item, todo);
  return item;
}

function updateTodoViewOnlyHint(snap) {
  // OQ-521: `dmuxai serve` (snapshot source "files") is a view-only dashboard;
  // to-do runs need `dmuxai --web` (TUI-attached dashboard, source "tui").
  var hint = $('todo-view-only');
  if (!hint) { return; }
  var viewOnly = Boolean(snap) && String(snap.source || '') === 'files';
  hint.hidden = !viewOnly;
}

function renderTodos(snap) {
  var list = $('todo-list');
  if (!list) { return; }
  updateTodoViewOnlyHint(snap);
  var todos = snap && Array.isArray(snap.todos) ? snap.todos : [];

  var i;
  var alive = {};
  for (i = 0; i < todos.length; i++) {
    var todo = todos[i];
    if (todo && todo.id) { alive[String(todo.id)] = true; }
  }
  if (todoEditId && !alive[todoEditId]) { todoEditId = null; }
  for (var key in todoItemErrors) {
    if (Object.prototype.hasOwnProperty.call(todoItemErrors, key) && !alive[key]) {
      delete todoItemErrors[key];
    }
  }

  // Preserve the DOM of the item being inline-edited so focus and unsaved
  // input survive snapshot re-renders; rebuild the other items (UX-540).
  var preserved = null;
  var oldById = {};
  for (i = 0; i < list.children.length; i++) {
    var child = list.children[i];
    var childId = child.getAttribute('data-todo-id');
    if (childId) { oldById[childId] = child; }
  }
  if (todoEditId) {
    var candidate = oldById[todoEditId];
    if (candidate && candidate.classList.contains('todo-editing')) {
      preserved = candidate;
    }
  }

  var frag = document.createDocumentFragment();
  var preservedTodo = null;
  for (var j = 0; j < todos.length; j++) {
    var current = todos[j];
    var tid = current && current.id ? String(current.id) : '';
    if (preserved && tid === todoEditId) {
      frag.appendChild(preserved);
      preservedTodo = current;
    } else {
      frag.appendChild(renderTodoItem(current));
    }
  }

  list.replaceChildren();
  if (todos.length === 0) {
    list.appendChild(el('p', 'empty', 'no to-dos yet'));
    return;
  }
  list.appendChild(frag);
  if (preservedTodo) {
    refreshItemErrorRow(preserved, todoEditId);
  }
}

function bindTodoForm() {
  $('todo-form').addEventListener('submit', function(e) {
    e.preventDefault();
    submitNewTodo();
  });
  var jiraInput = $('todo-jira');
  if (jiraInput) {
    jiraInput.addEventListener('input', updateJiraPreview);
  }
}

// ---------------------------------------------------------------------------
// Import from Jira/Confluence (SPEC-web-dashboard FR-561 / FR-562 / UX-560,
// TC-568). The form appears only when at least one enabled MCP server of kind
// "jira" or "confluence" is configured (GET /api/mcp/servers); imports land in
// the to-do list with their source badge and source_ref (kept as text).
// ---------------------------------------------------------------------------

var importServers = [];

function importCapableServers(servers) {
  var out = [];
  for (var i = 0; i < servers.length; i++) {
    var server = servers[i] || {};
    if (
      (server.kind === 'jira' || server.kind === 'confluence') &&
      server.enabled !== false
    ) {
      out.push(server);
    }
  }
  return out;
}

function loadImportServers() {
  todoApi('GET', '/api/mcp/servers')
    .then(function(payload) {
      // FR-562 shape is {"servers":[...]}; tolerate a bare array too.
      var servers = payload && Array.isArray(payload.servers)
        ? payload.servers
        : (Array.isArray(payload) ? payload : []);
      importServers = importCapableServers(servers);
      renderImportPanel();
    })
    .catch(function(err) {
      if (err && err.fatal) {
        onUnauthorized();
        return;
      }
      // Without a server list the import panel simply stays hidden; the board
      // and graph keep working. Warn for diagnosis only.
      console.warn('dashboard: MCP server list unavailable: ' + err.message);
    });
}

function renderImportPanel() {
  var panel = $('import-panel');
  var select = $('import-server');
  if (!panel || !select) { return; }
  select.replaceChildren();
  if (importServers.length === 0) {
    panel.hidden = true;
    return;
  }
  for (var i = 0; i < importServers.length; i++) {
    var server = importServers[i];
    var name = String(server.name || '');
    var opt = document.createElement('option');
    opt.value = name;
    opt.textContent = name + ' (' + String(server.kind || '') + ')';
    select.appendChild(opt);
  }
  panel.hidden = false;
}

function setImportBusy(busy) {
  var button = $('import-submit');
  if (button) { button.disabled = busy; }
}

function showImportStatus(text, isError) {
  var status = $('import-status');
  if (!status) { return; }
  status.hidden = false;
  status.textContent = text;
  status.classList.toggle('import-status-error', Boolean(isError));
}

function submitImport() {
  var select = $('import-server');
  var queryInput = $('import-query');
  var server = select.value;
  if (!server) { return; }
  setImportBusy(true);
  todoApi('POST', '/api/import', { server: server, query: queryInput.value.trim() })
    .then(function(result) {
      setImportBusy(false);
      var imported = result && typeof result.imported === 'number' ? result.imported : 0;
      var updated = result && typeof result.updated === 'number' ? result.updated : 0;
      showImportStatus('imported ' + imported + ', updated ' + updated, false);
      refreshTodos();
    })
    .catch(function(err) {
      setImportBusy(false);
      if (err && err.fatal) {
        onUnauthorized();
        return;
      }
      // Inline error from the server's {"error": ...} body (UX-560).
      showImportStatus(err.message, true);
    });
}

function bindImportForm() {
  var form = $('import-form');
  if (!form) { return; }
  form.addEventListener('submit', function(e) {
    e.preventDefault();
    submitImport();
  });
}

function render(nowMs) {
  var snap = snapshot;
  if (!snap || typeof snap.revision !== 'number') { return; }

  $('workspace').textContent = String(snap.workspace || '-');
  $('source').textContent = String(snap.source || '-');
  $('revision').textContent = String(snap.revision);

  renderAgents(snap.agents);

  var graph = snap.graph;
  if (!graph) {
    $('graph-empty').hidden = false;
    $('graph-body').hidden = true;
    renderGateHint(null);
    renderCounts(null);
    renderBar(null);
  } else {
    $('graph-empty').hidden = true;
    $('graph-body').hidden = false;
    $('graph-id').textContent = String(graph.id || '');
    $('graph-phase').textContent = String(graph.phase || '');
    $('graph-model').textContent = String(
      graph.orchestrator_model ? 'orchestrator: ' + graph.orchestrator_model : ''
    );
    $('graph-prompt').textContent = String(graph.user_prompt || '');
    $('graph-prompt').title = String(graph.user_prompt || '');
    renderCounts(graph);
    renderBar(graph);
    renderNodes(graph, nowMs);
    renderGateHint(graph);
  }
}

function scheduleTick() {
  if (tickTimer !== null) { return; }
  tickTimer = setInterval(function() {
    var snap = snapshot;
    if (!snap || typeof lastRendered !== 'number') { return; }
    render(lastRendered);
  }, TICK_MS);
}

function onSnapshotEvent(e) {
  var snap;
  try {
    snap = JSON.parse(e.data);
  } catch (err) {
    showMsg('dashboard: ignoring malformed snapshot event', true);
    return;
  }
  if (!snap || snap.schema !== 'dmuxai.dashboard/v1' || typeof snap.revision !== 'number') {
    showMsg('dashboard: ignoring snapshot with unexpected schema', true);
    return;
  }
  hideMsg();
  sseFailures = 0;
  setConn('live', 'live');
  applySnapshot(snap);
}

function applySnapshot(snap) {
  snap.receivedAt = Date.now();
  snapshot = snap;
  lastRendered = Date.now();
  applySnapshotUi(snap);
  render(lastRendered);
  renderTodos(snap);
  scheduleTick();
}

function onUnauthorized() {
  storageRemove(TOKEN_KEY);
  stopSse();
  stopPolling();
  stopTickTimer();
  setConn('error', 'unauthorized');
  showMsg(
    'Access token rejected. The token rotates on every server start; ' +
    'open the dashboard URL that dmuxai printed (it contains ?token=...).',
    true
  );
}

function startSse() {
  if (typeof EventSource === 'undefined') {
    startPolling();
    return;
  }
  stopSse();
  sseFailures = 0;
  setConn('connecting', 'connecting');
  try {
    sse = new EventSource('/api/events?token=' + encodeURIComponent(token));
  } catch (err) {
    showMsg('dashboard: SSE unavailable, falling back to polling', true);
    startPolling();
    return;
  }

  sse.addEventListener('snapshot', onSnapshotEvent);
  sse.addEventListener('error', function() {
    sseFailures += 1;
    if (sse && sse.readyState === EventSource.CLOSED) {
      showMsg('dashboard: SSE closed, falling back to polling', true);
      startPolling();
      return;
    }
    if (sseFailures >= MAX_SSE_FAILURES) {
      showMsg('dashboard: SSE unstable, falling back to polling', true);
      startPolling();
    }
  });
}

function stopSse() {
  if (sse) {
    sse.close();
    sse = null;
  }
}

function stopTickTimer() {
  if (tickTimer !== null) {
    clearInterval(tickTimer);
    tickTimer = null;
  }
}

function startPolling() {
  if (pollTimer !== null) { return; }
  stopSse();
  pollDelay = POLL_INITIAL_MS;
  setConn('poll', 'polling');
  schedulePoll();
}

function schedulePoll() {
  pollTimer = window.setTimeout(function() {
    pollTimer = null;
    poll();
    schedulePoll();
  }, pollDelay);
}

function stopPolling() {
  if (pollTimer !== null) {
    clearTimeout(pollTimer);
    pollTimer = null;
  }
}

function poll() {
  fetch('/api/state', {
    headers: { Authorization: 'Bearer ' + token },
    cache: 'no-store'
  })
    .then(function(res) {
      if (res.status === 401) {
        var fatal = new Error('unauthorized');
        fatal.fatal = true;
        throw fatal;
      }
      if (!res.ok) {
        throw new Error('HTTP ' + res.status);
      }
      return res.json();
    })
    .then(function(snap) {
      if (!snap || snap.schema !== 'dmuxai.dashboard/v1' || typeof snap.revision !== 'number') {
        throw new Error('bad snapshot schema');
      }
      sseFailures = 0;
      pollDelay = POLL_INITIAL_MS;
      hideMsg();
      applySnapshot(snap);
    })
    .catch(function(err) {
      if (err && err.fatal) {
        // Token rotation invalidates this page; stop everything and tell the
        // user to reopen the URL dmuxai printed.
        onUnauthorized();
        return;
      }
      showMsg('dashboard: poll failed (' + err.message + ')', true);
      pollDelay = Math.min(pollDelay * 2, POLL_MAX_MS);
    });
}

window.addEventListener('pagehide', function() {
  stopSse();
  stopPolling();
  stopTickTimer();
});

function init() {
  // Theme controls work even without a token (the panel is header-visible).
  bindThemeControls();
  // A stored choice counts as already-made: snapshot values must not override it.
  var stored = storedTheme();
  userThemeChosen = validTheme(stored);
  themePref = userThemeChosen ? stored : 'system';
  applyResolvedTheme();
  setThemeSelect();

  var accent = storedAccent();
  if (isHexColor(accent)) {
    applyAccentOverride(accent);
    userAccentChosen = true;
  }
  syncAccentPicker();

  if (colorSchemeMedia && typeof colorSchemeMedia.addEventListener === 'function') {
    colorSchemeMedia.addEventListener('change', colorSchemeListener);
  }

  if (!token) {
    showMsg(
      'Missing access token. Start DmuxAI and open the dashboard URL it prints ' +
      '(for example: dmuxai serve, then open http://127.0.0.1:7420/?token=...).',
      true
    );
    return;
  }

  render(Date.now());
  bindTodoForm();
  bindImportForm();
  loadImportServers();
  startSse();
}

init();
