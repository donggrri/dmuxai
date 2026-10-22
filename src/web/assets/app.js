'use strict';

var TICK_MS = 1000;
var POLL_INITIAL_MS = 2000;
var POLL_MAX_MS = 30000;
var MAX_SSE_FAILURES = 3;
var JOB_PREVIEW_LEN = 120;
var TOKEN_KEY = 'dmuxai.token';
// UX-700: progress-bar segment order is completed (green), running (striped
// blue), failed (red), then the remaining states with pending last, matching
// mockup #5.
var STATUS_ORDER = [
  'completed', 'running', 'failed', 'pending', 'ready', 'waiting_gate', 'skipped'
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

var baseDocumentTitle = document.title || 'DmuxAI dashboard';
var lastObservedRunState = 'idle';

function updateFavicon(state) {
  var link = document.getElementById('favicon') || document.querySelector('link[rel~="icon"]');
  if (!link) { return; }
  var href = '/assets/favicon-idle.svg';
  if (state === 'running') {
    href = '/assets/favicon-running.svg';
  } else if (state === 'done') {
    href = '/assets/favicon-done.svg';
  } else if (state === 'failed') {
    href = '/assets/favicon-failed.svg';
  }
  if (link.getAttribute('href') !== href) {
    link.setAttribute('href', href);
  }
}

if (typeof window.addEventListener === 'function') {
  window.addEventListener('focus', function () {
    if (document.title.indexOf('✓ ') === 0 || document.title.indexOf('✗ ') === 0) {
      document.title = baseDocumentTitle;
      lastObservedRunState = 'idle';
      updateFavicon('idle');
    }
  });
}

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
  // [theme] reduced_motion / DMUXAI_REDUCED_MOTION (UX-821) also stills the web spinners.
  document.documentElement.classList.toggle('reduced-motion', !!(ui && ui.reduced_motion));
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
    history.replaceState(null, '', location.pathname + location.hash);
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
  var b = el('span', 'badge badge-' + label);
  if (label === 'running') {
    b.appendChild(el('span', 'spinner'));
    b.appendChild(document.createTextNode(' ' + label));
  } else {
    b.textContent = label;
  }
  return b;
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

// Elapsed ms of a running agent run, corrected for server/browser clock skew
// the same way as nodeElapsed (FR-823).
function runElapsedMs(run, nowMs) {
  if (!run || typeof run.started_at_ms !== 'number') { return 0; }
  var snap = snapshot;
  if (snap && typeof snap.generated_at_ms === 'number' && typeof snap.receivedAt === 'number') {
    var serverNow = typeof snap.sent_at_ms === 'number' && snap.sent_at_ms > 0
      ? snap.sent_at_ms
      : snap.generated_at_ms;
    return Math.max(0, (serverNow - run.started_at_ms) + (nowMs - snap.receivedAt));
  }
  return Math.max(0, nowMs - run.started_at_ms);
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
      var serverNow = typeof snap.sent_at_ms === 'number' && snap.sent_at_ms > 0
      ? snap.sent_at_ms
      : snap.generated_at_ms;
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
    meta.appendChild(el('span', 'node-runner', runnerShortName(node.runner)));
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

// ---------------------------------------------------------------------------
// Graph view: horizontal swimlane timeline (SPEC-graph-swimlane FR-880..885,
// UX-880, UX-881; replaces the SPEC-graph-view subway drawing).
// One lane per runner, time runs left to right, each node is a card placed at
// its start and as wide as its duration, a "now" line, estimated starts on
// pending cards, and dependency arrows that never loop back over their source.
// Inline SVG only, no external resources, colors from theme CSS variables.
// ---------------------------------------------------------------------------

var GRAPH_VIEW_KEY = 'dmuxai.graphView';
// Standard SVG namespace; not a fetched resource.
var SVG_NS = 'http://www.w3.org/2000/svg';
var GRAPH_MIN_SCALE = 0.25;
var GRAPH_MAX_SCALE = 3;
var LANE_LABEL_W = 170;
var LANE_ROW_H = 104;
var CARD_H = 56;
var CARD_MIN_W = 120;
var AXIS_H = 30;
var TIMELINE_PAD_R = 28;
var TIMELINE_MIN_W = 640;
// FR-881 / FR-884: cards closer than this stack into sub-rows, and arrows
// with less room than this take the orthogonal route through the lane gap.
var SUBROW_GAP = 24;
var ARROW_MIN_GAP = 24;
// UX-881: the smallest SVG font size the swimlane uses, and the rendered
// floor Fit never goes below (so Fit never scales under 1).
var GRAPH_MIN_FONT = 11;
var GRAPH_TEXT_FLOOR = 11;
var GRAPH_DEFAULT_EST_MS = 120000;
var GRAPH_MIN_H = 180;
var GRAPH_MAX_H_RATIO = 0.7;
// Running timelines are rebuilt this often so the now line and tails move.
var GRAPH_REBUILD_MS = 5000;
var GRAPH_LANE_COLORS = 6;
var GRAPH_TICK_STEPS = [
  30000, 60000, 120000, 300000, 600000, 900000, 1800000,
  3600000, 7200000, 10800000, 21600000, 43200000, 86400000
];

// graphViewPref is the stored user choice (or null before one exists, in which
// case the default follows the layer count: Graph when layers > 1).
var graphViewPref = null;
var graphViewMode = 'graph';
var graphTransform = { scale: 1, x: 0, y: 0 };
var graphSelectedId = null;
// UX-936: selected checklist task highlights swimlane nodes (survives snapshot re-render).
var taskChecklistSelectedId = null;
var taskChecklistHighlight = {};
var graphPan = null;
var graphPointerDown = false;
var graphZoomGroup = null;
// Set once the user pans/zooms; Fit re-enables auto-fit (UX-703).
var graphUserInteracted = false;
var lastGraphGeometryKey = null;
var lastGraphCanvasRevision = null;
var lastGraphCanvasGeometryKey = null;
var lastGraphCanvasNodesKey = null;
var graphGeometry = null;
var graphLastBuildAt = 0;

function svgEl(tag) { return document.createElementNS(SVG_NS, tag); }

function svgText(x, y, text, className, anchor) {
  var node = svgEl('text');
  node.setAttribute('x', x);
  node.setAttribute('y', y);
  node.setAttribute('text-anchor', anchor || 'start');
  node.setAttribute('class', className);
  node.textContent = text;
  return node;
}

function svgRect(className, x, y, w, h, rx) {
  var rect = svgEl('rect');
  rect.setAttribute('class', className);
  rect.setAttribute('x', x);
  rect.setAttribute('y', y);
  rect.setAttribute('width', Math.max(0, w));
  rect.setAttribute('height', Math.max(0, h));
  if (rx) { rect.setAttribute('rx', rx); }
  return rect;
}

function svgLine(className, x1, y1, x2, y2) {
  var line = svgEl('line');
  line.setAttribute('class', className);
  line.setAttribute('x1', x1);
  line.setAttribute('y1', y1);
  line.setAttribute('x2', x2);
  line.setAttribute('y2', y2);
  return line;
}

function graphNodeStatus(node) {
  return node && typeof node.status === 'string' ? node.status : 'pending';
}

function graphIsFinished(status) {
  return status === 'completed' || status === 'failed' || status === 'skipped';
}

function graphIsWaiting(status) {
  return status === 'pending' || status === 'ready' || status === 'waiting_gate';
}

// UX-700: collapse long runner ids to the short label (`opencode-glm` -> `glm`,
// `agy` -> `AGY`). Lane labels keep the full runner name.
function runnerShortName(name) {
  var short = String(name || '').replace(/^opencode-/i, '');
  if (short.toLowerCase() === 'agy') { return 'AGY'; }
  return short;
}

function runnerLaneName(name) {
  var full = String(name || '');
  return full.toLowerCase() === 'agy' ? 'AGY' : (full || 'runner');
}

// Status words shown on the card chip and in place of an unknown elapsed time.
var STATION_STATE_WORDS = {
  completed: 'done',
  failed: 'failed',
  running: 'running',
  pending: 'pending',
  ready: 'pending',
  waiting_gate: 'waiting',
  skipped: 'skipped'
};

function stationStateWord(status) {
  return STATION_STATE_WORDS[status] || 'pending';
}

var CARD_ICONS = {
  completed: '\u2713',
  failed: '\u00d7',
  skipped: '\u2013',
  waiting_gate: '\u23f8',
  pending: '\u25cb',
  ready: '\u25cb'
};

function nodeAriaLabel(node) {
  return String(node.id) + ': ' + String(node.role || '') + ', ' +
    runnerShortName(node.runner) + ', ' + graphNodeStatus(node);
}

function nodeTitleText(node) {
  return String(node.id) + ' \u2014 ' + String(node.role || '') + ' (' +
    runnerShortName(node.runner) + ')';
}

function graphNodeElementById(id) {
  var groups = document.querySelectorAll('.graph-svg g.node[data-id]');
  for (var i = 0; i < groups.length; i++) {
    if (groups[i].getAttribute('data-id') === id) { return groups[i]; }
  }
  return null;
}

// FR-882: "now" on the server clock, from the snapshot's send time plus the
// time this page has held it (the same correction as runElapsedMs). A
// negative offset (browser clock behind) clamps to 0.
function graphServerNow(snap, nowMs) {
  var now = typeof nowMs === 'number' ? nowMs : Date.now();
  if (snap && typeof snap.generated_at_ms === 'number' && typeof snap.receivedAt === 'number') {
    var serverNow = typeof snap.sent_at_ms === 'number' && snap.sent_at_ms > 0
      ? snap.sent_at_ms
      : snap.generated_at_ms;
    return serverNow + Math.max(0, now - snap.receivedAt);
  }
  return now;
}

function graphMedian(values) {
  var sorted = values.slice().sort(function (a, b) { return a - b; });
  var mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : Math.round((sorted[mid - 1] + sorted[mid]) / 2);
}

function graphFinishedDuration(node) {
  if (typeof node.started_at_ms === 'number' && typeof node.finished_at_ms === 'number') {
    return Math.max(0, node.finished_at_ms - node.started_at_ms);
  }
  if (typeof node.elapsed_ms === 'number') { return Math.max(0, node.elapsed_ms); }
  return null;
}

// Dependencies of every node: `deps` from the snapshot plus `edges`.
function graphDepsMap(graph) {
  var nodes = graph && Array.isArray(graph.nodes) ? graph.nodes : [];
  var edges = graph && Array.isArray(graph.edges) ? graph.edges : [];
  var deps = {};
  var i;
  function add(to, from) {
    var list = deps[to] || (deps[to] = []);
    if (from !== to && list.indexOf(from) === -1) { list.push(from); }
  }
  for (i = 0; i < nodes.length; i++) {
    var id = String(nodes[i].id);
    if (!deps[id]) { deps[id] = []; }
    var own = Array.isArray(nodes[i].deps) ? nodes[i].deps : [];
    for (var d = 0; d < own.length; d++) { add(id, String(own[d])); }
  }
  for (i = 0; i < edges.length; i++) {
    if (edges[i] && edges[i].from != null && edges[i].to != null) {
      add(String(edges[i].to), String(edges[i].from));
    }
  }
  return deps;
}

// FR-882: start/end of every node on the server clock. Timed nodes use their
// timestamps; running nodes end at now on the card and at
// max(now, start + estimate) for their dependents; pending nodes start at
// max(now, latest dependency end) and last the estimate (median finished
// duration of the same role, else of all finished nodes, else 2 minutes).
function graphEstimate(graph, serverNow) {
  var nodes = graph && Array.isArray(graph.nodes) ? graph.nodes : [];
  var depsMap = graphDepsMap(graph);
  var byId = {};
  var byRole = {};
  var all = [];
  var knownMin = null;
  var i;
  for (i = 0; i < nodes.length; i++) {
    var n = nodes[i];
    byId[String(n.id)] = n;
    if (typeof n.started_at_ms === 'number') {
      knownMin = knownMin === null ? n.started_at_ms : Math.min(knownMin, n.started_at_ms);
    }
    if (graphIsFinished(graphNodeStatus(n))) {
      var dur = graphFinishedDuration(n);
      if (dur !== null) {
        all.push(dur);
        var role = String(n.role || '');
        (byRole[role] = byRole[role] || []).push(dur);
      }
    }
  }
  function estDuration(node) {
    var same = byRole[String(node.role || '')];
    if (same && same.length) { return graphMedian(same); }
    if (all.length) { return graphMedian(all); }
    return GRAPH_DEFAULT_EST_MS;
  }
  var out = {};
  var visiting = {};
  function resolve(id) {
    if (out[id]) { return out[id]; }
    var node = byId[id];
    if (!node || visiting[id]) { return null; }
    visiting[id] = true;
    var status = graphNodeStatus(node);
    var est = estDuration(node);
    var depEnd = null;
    var blocked = false;
    var deps = depsMap[id] || [];
    for (var d = 0; d < deps.length; d++) {
      var depId = deps[d];
      var dep = resolve(depId);
      if (!dep) { continue; }
      if (graphNodeStatus(byId[depId]) === 'failed') { blocked = true; }
      depEnd = depEnd === null ? dep.planEnd : Math.max(depEnd, dep.planEnd);
    }
    var hasStart = typeof node.started_at_ms === 'number';
    var hasFinish = typeof node.finished_at_ms === 'number';
    var r = { estDurMs: est, estimated: false, blocked: false, untimed: false };
    if (status === 'running') {
      r.start = hasStart ? node.started_at_ms : (depEnd !== null ? Math.min(depEnd, serverNow) : serverNow);
      r.untimed = !hasStart;
      r.end = Math.max(serverNow, r.start);
      r.planEnd = Math.max(serverNow, r.start + est);
    } else if (graphIsFinished(status) && hasStart && hasFinish) {
      r.start = node.started_at_ms;
      r.end = Math.max(node.finished_at_ms, r.start);
      r.planEnd = r.end;
    } else if (status === 'skipped' || (status === 'failed' && !hasStart)) {
      // Never ran: counts as ending now (or at its finish time).
      r.start = depEnd !== null ? Math.min(depEnd, serverNow) : serverNow;
      r.end = hasFinish ? Math.max(node.finished_at_ms, r.start) : r.start;
      r.planEnd = hasFinish ? r.end : serverNow;
      r.untimed = true;
    } else if (status === 'completed' || status === 'failed') {
      // Finished without timestamps (older runs): place it after its
      // dependencies, or just before the earliest timed node, with the estimate.
      var origin = knownMin !== null ? Math.min(knownMin, serverNow) : serverNow;
      r.start = depEnd !== null ? depEnd : origin - est;
      r.end = r.start + est;
      r.planEnd = r.end;
      r.untimed = true;
    } else {
      r.start = Math.max(serverNow, depEnd !== null ? depEnd : serverNow);
      r.end = r.start + est;
      r.planEnd = r.end;
      r.estimated = true;
      r.blocked = blocked;
    }
    visiting[id] = false;
    out[id] = r;
    return r;
  }
  for (i = 0; i < nodes.length; i++) { resolve(String(nodes[i].id)); }
  return out;
}

// FR-880: the smallest "nice" step giving at most 10 intervals (11 tick
// marks counting both ends); runs shorter than three minutes use 30s steps.
function graphTickStep(spanMs) {
  if (spanMs < 180000) { return 30000; }
  for (var i = 0; i < GRAPH_TICK_STEPS.length; i++) {
    if (spanMs / GRAPH_TICK_STEPS[i] <= 10) { return GRAPH_TICK_STEPS[i]; }
  }
  return GRAPH_TICK_STEPS[GRAPH_TICK_STEPS.length - 1];
}

function fmtTick(ms) {
  var total = Math.round(ms / 1000);
  var h = Math.floor(total / 3600);
  var m = Math.floor((total % 3600) / 60);
  var s = total % 60;
  if (h > 0) { return h + 'h' + (m ? m + 'm' : ''); }
  if (m > 0) { return m + 'm' + (s ? s + 's' : ''); }
  return s === 0 ? '0m' : s + 's';
}

function graphTimelineWidth() {
  var canvas = $('graph-canvas');
  var cw = canvas && canvas.clientWidth ? canvas.clientWidth : 1000;
  return Math.max(TIMELINE_MIN_W, cw - LANE_LABEL_W - TIMELINE_PAD_R - 2);
}

// FR-880..FR-883: lanes, cards (with sub-rows), axis and now line positions.
function buildSwimlaneGeometry(graph, serverNow, timelineW, pass) {
  var nodes = graph && Array.isArray(graph.nodes) ? graph.nodes : [];
  var est = graphEstimate(graph, serverNow);
  var phase = graph && typeof graph.phase === 'string' ? graph.phase : 'active';
  var ended = phase === 'completed' || phase === 'failed';
  var entries = [];
  var i;
  var t0 = null;
  var lastEnd = null;
  for (i = 0; i < nodes.length; i++) {
    var id = String(nodes[i].id);
    var e = est[id];
    if (!e) { continue; }
    entries.push({ node: nodes[i], id: id, index: i, est: e });
    t0 = t0 === null ? e.start : Math.min(t0, e.start);
    var reach = Math.max(e.end, e.planEnd);
    lastEnd = lastEnd === null ? reach : Math.max(lastEnd, reach);
  }
  if (t0 === null) { t0 = serverNow; }
  if (lastEnd === null) { lastEnd = serverNow; }
  var endAt = ended ? lastEnd : Math.max(serverNow, lastEnd);
  var span = Math.max(60000, endAt - t0);
  if (!isFinite(span)) { span = 60000; }
  var tickStep = graphTickStep(span);
  var spanDraw = span * 1.04;
  var pxPerMs = timelineW / spanDraw;
  function xAt(t) { return LANE_LABEL_W + (t - t0) * pxPerMs; }

  // Lanes in first-use order (by start, then plan order).
  entries.sort(function (a, b) {
    if (a.est.start !== b.est.start) { return a.est.start - b.est.start; }
    return a.index - b.index;
  });
  var lanes = [];
  var laneByRunner = {};
  for (i = 0; i < entries.length; i++) {
    var entry = entries[i];
    var runner = String(entry.node.runner || '');
    var lane = laneByRunner[runner];
    if (!lane) {
      lane = { runner: runner, index: lanes.length, entries: [], rows: [], model: '' };
      laneByRunner[runner] = lane;
      lanes.push(lane);
    }
    lane.entries.push(entry);
    if (entry.node.model_id) { lane.model = String(entry.node.model_id); }
    entry.x = xAt(entry.est.start);
    var durMs = Math.max(0, entry.est.end - entry.est.start);
    entry.w = Math.max(graphCardMinWidth(entry, t0), durMs * pxPerMs);
    entry.h = CARD_H;
    entry.lane = lane.index;
    // FR-881: stack into the first sub-row with at least SUBROW_GAP of room.
    var row = 0;
    while (row < lane.rows.length && lane.rows[row] + SUBROW_GAP > entry.x) { row += 1; }
    lane.rows[row] = entry.x + entry.w;
    entry.subrow = row;
  }

  var top = AXIS_H;
  var maxRight = LANE_LABEL_W + timelineW;
  var positions = {};
  for (i = 0; i < lanes.length; i++) {
    lanes[i].top = top;
    lanes[i].height = Math.max(1, lanes[i].rows.length) * LANE_ROW_H;
    for (var j = 0; j < lanes[i].entries.length; j++) {
      var ent = lanes[i].entries[j];
      ent.y = top + ent.subrow * LANE_ROW_H + (LANE_ROW_H - CARD_H) / 2;
      positions[ent.id] = ent;
      maxRight = Math.max(maxRight, ent.x + ent.w);
    }
    top += lanes[i].height;
  }

  // Minimum card widths can push the last cards past the axis; shrink the
  // time scale until they fit inside the requested width.
  var overflow = maxRight - (LANE_LABEL_W + timelineW);
  var passNo = pass || 0;
  if (overflow > 1 && passNo < 2 && timelineW - overflow >= TIMELINE_MIN_W / 2) {
    return buildSwimlaneGeometry(graph, serverNow, timelineW - overflow, passNo + 1);
  }

  var nowT = ended ? endAt : serverNow;
  return {
    entries: entries,
    positions: positions,
    lanes: lanes,
    width: Math.ceil(maxRight + TIMELINE_PAD_R),
    height: top + 2,
    t0: t0,
    timelineW: timelineW,
    pxPerMs: pxPerMs,
    spanDraw: spanDraw,
    tickStep: tickStep,
    ended: ended,
    nowX: xAt(nowT),
    nowLabel: (ended ? 'ended ' : 'now ') + fmtElapsed(Math.max(0, nowT - t0)),
    serverNow: serverNow
  };
}

// Text metrics for the card layout (SVG units, matching the CSS font sizes).
var CARD_ID_CHAR = 8;
var CARD_SUB_CHAR = 6.4;
var CARD_TIME_CHAR = 7.2;
var CARD_CHIP_CHAR = 7;
var CARD_MAX_MIN_W = 260;

function graphChipText(entry) {
  if (entry.est.blocked) { return 'BLOCKED'; }
  return stationStateWord(graphNodeStatus(entry.node)).toUpperCase();
}

// FR-881: a card is at least wide enough for its id and status chip, and for
// its second line (role or estimate, and time), whichever is wider.
function graphCardMinWidth(entry, t0) {
  var chipW = graphChipText(entry).length * CARD_CHIP_CHAR + 12;
  var tailW = graphNodeStatus(entry.node) === 'running' ? 30 : 0;
  var top = 26 + String(entry.id).length * CARD_ID_CHAR + 8 + chipW + 8 + tailW;
  var sub = graphCardSub(entry, { t0: t0 });
  var time = graphCardTime(entry, null);
  var bottom = 12 + sub.length * CARD_SUB_CHAR + 14 + time.length * CARD_TIME_CHAR + 10;
  return Math.min(CARD_MAX_MIN_W, Math.max(CARD_MIN_W, top, bottom));
}

// FR-884: an arrow from the right edge of the source card to the left edge of
// the target. With at least ARROW_MIN_GAP of room it is a straight line (same
// row) or a cubic curve. Otherwise it leaves the source's bottom (or top),
// runs through the gap between rows, and enters the target from the left, so
// it never curves back over the source card. Its long vertical leg is moved
// to a column free of other cards when `obstacles` (card rects) are given.
function graphArrowPath(s, t, obstacles) {
  var x1 = s.x + s.w;
  var y1 = s.y + s.h / 2;
  var x2 = t.x;
  var y2 = t.y + t.h / 2;
  if (x2 - x1 >= ARROW_MIN_GAP && !graphDirectBlocked(x1, y1, x2, y2, obstacles, s, t)) {
    if (Math.abs(y2 - y1) < 1) { return 'M' + x1 + ',' + y1 + ' L' + x2 + ',' + y2; }
    var dx = (x2 - x1) / 2;
    return 'M' + x1 + ',' + y1 + ' C' + (x1 + dx) + ',' + y1 + ' ' +
      (x2 - dx) + ',' + y2 + ' ' + x2 + ',' + y2;
  }
  var down = t.y >= s.y;
  var gap = (LANE_ROW_H - CARD_H) / 2;
  var sx = s.x + Math.min(24, s.w / 2);
  var sy = down ? s.y + s.h : s.y;
  var gapS = down ? s.y + s.h + gap : s.y - gap;
  var gapT = down ? t.y - gap : t.y + t.h + gap;
  var vx = x2 - 12;
  var xv = graphFreeColumn(vx, sx, Math.min(gapS, gapT), Math.max(gapS, gapT), obstacles, s, t);
  if (xv === vx) {
    return 'M' + sx + ',' + sy + ' L' + sx + ',' + gapS + ' L' + vx + ',' + gapS +
      ' L' + vx + ',' + y2 + ' L' + x2 + ',' + y2;
  }
  return 'M' + sx + ',' + sy + ' L' + sx + ',' + gapS + ' L' + xv + ',' + gapS +
    ' L' + xv + ',' + gapT + ' L' + vx + ',' + gapT + ' L' + vx + ',' + y2 +
    ' L' + x2 + ',' + y2;
}

// Whether the direct line or curve from (x1,y1) to (x2,y2) (same control
// points as graphArrowPath) passes over a card other than its source and
// target; such arrows take the routed path through the row gaps instead.
function graphDirectBlocked(x1, y1, x2, y2, obstacles, s, t) {
  if (!obstacles || !obstacles.length) { return false; }
  var dx = (x2 - x1) / 2;
  for (var k = 1; k < 24; k++) {
    var u = k / 24;
    var v = 1 - u;
    var px = v * v * v * x1 + 3 * v * v * u * (x1 + dx) + 3 * v * u * u * (x2 - dx) + u * u * u * x2;
    var py = v * v * v * y1 + 3 * v * v * u * y1 + 3 * v * u * u * y2 + u * u * u * y2;
    for (var i = 0; i < obstacles.length; i++) {
      var o = obstacles[i];
      if (o === s || o === t) { continue; }
      if (px > o.x - 2 && px < o.x + o.w + 2 && py > o.y - 2 && py < o.y + o.h + 2) { return true; }
    }
  }
  return false;
}

// The x closest to `preferred` whose vertical segment [ya, yb] crosses no
// card other than the arrow's own source and target.
function graphFreeColumn(preferred, alt, ya, yb, obstacles, s, t) {
  if (!obstacles || !obstacles.length) { return preferred; }
  function blocked(x) {
    for (var i = 0; i < obstacles.length; i++) {
      var o = obstacles[i];
      if (o === s || o === t) { continue; }
      if (x > o.x - 2 && x < o.x + o.w + 2 && yb > o.y && ya < o.y + o.h) { return true; }
    }
    return false;
  }
  if (!blocked(preferred)) { return preferred; }
  var candidates = [alt];
  for (var i = 0; i < obstacles.length; i++) {
    candidates.push(obstacles[i].x - 12, obstacles[i].x + obstacles[i].w + 12);
  }
  var best = preferred;
  var bestDist = Infinity;
  for (var c = 0; c < candidates.length; c++) {
    var x = candidates[c];
    if (x < LANE_LABEL_W + 4 || blocked(x)) { continue; }
    var dist = Math.abs(x - preferred);
    if (dist < bestDist) { best = x; bestDist = dist; }
  }
  return best;
}

// The shared arrowhead of a node takes the most blocking source state:
// failed, then running, then any other unfinished state, then completed.
function graphHeadState(states) {
  if (states.indexOf('failed') !== -1) { return 'failed'; }
  if (states.indexOf('running') !== -1) { return 'running'; }
  for (var i = 0; i < states.length; i++) {
    if (states[i] !== 'completed') { return states[i]; }
  }
  return 'completed';
}

function graphArrowHead(t, className) {
  var x = t.x;
  var y = t.y + t.h / 2;
  var head = svgEl('path');
  head.setAttribute('class', className);
  head.setAttribute('d', 'M' + (x - 7) + ',' + (y - 4) + ' L' + x + ',' + y +
    ' L' + (x - 7) + ',' + (y + 4) + ' Z');
  return head;
}

function graphFitText(text, maxPx, charPx) {
  var str = String(text || '');
  var max = Math.max(1, Math.floor(maxPx / charPx));
  if (str.length <= max) { return str; }
  return max <= 1 ? '\u2026' : str.slice(0, max - 1) + '\u2026';
}

function graphCardTime(entry, nowMs) {
  var node = entry.node;
  var status = graphNodeStatus(node);
  if (graphIsWaiting(status)) { return '~' + fmtElapsed(entry.est.estDurMs); }
  var ms = nodeElapsed(node, snapshot, nowMs);
  if (ms === null && !entry.est.untimed) { ms = Math.max(0, entry.est.end - entry.est.start); }
  return ms === null ? stationStateWord(status) : fmtElapsed(ms);
}

function graphCardSub(entry, geometry) {
  var node = entry.node;
  var status = graphNodeStatus(node);
  if (graphIsWaiting(status)) {
    return 'est. start +' + fmtElapsed(Math.max(0, entry.est.start - geometry.t0));
  }
  if (status === 'failed' && typeof node.attempt === 'number' && node.attempt > 1) {
    return 'try ' + node.attempt + ' \u00b7 ' + String(node.role || '');
  }
  return String(node.role || '');
}

// FR-881: one card. Running cards get a pulsing dot and a striped tail.
function graphCard(entry, geometry, nowMs) {
  var node = entry.node;
  var status = graphNodeStatus(node);
  var group = svgEl('g');
  var classes = 'node card state-' + status;
  if (entry.est.blocked) { classes += ' card-blocked'; }
  if (graphSelectedId === entry.id) { classes += ' node-selected'; }
  if (taskChecklistHighlight[entry.id]) { classes += ' node-task-highlight'; }
  group.setAttribute('class', classes);
  group.setAttribute('data-id', entry.id);
  group.setAttribute('data-lane', String(entry.lane));
  group.setAttribute('data-subrow', String(entry.subrow));
  group.setAttribute('data-runner', String(node.runner || ''));
  group.setAttribute('transform', 'translate(' + entry.x + ',' + entry.y + ')');
  group.setAttribute('tabindex', '0');
  group.setAttribute('role', 'button');
  group.setAttribute('aria-label', nodeAriaLabel(node));

  var title = svgEl('title');
  title.textContent = nodeTitleText(node);
  group.appendChild(title);

  var w = entry.w;
  group.appendChild(svgRect('card-box', 0, 0, w, CARD_H, 8));
  group.appendChild(svgRect('card-accent', 0, 0, 4, CARD_H, 2));
  if (status === 'running') {
    var tail = svgRect('card-tail', w - 34, 1, 33, CARD_H - 2, 7);
    tail.setAttribute('fill', 'url(#graph-stripe)');
    group.appendChild(tail);
    var dot = svgEl('circle');
    dot.setAttribute('class', 'run-dot');
    dot.setAttribute('cx', 16);
    dot.setAttribute('cy', 18);
    dot.setAttribute('r', 4);
    group.appendChild(dot);
  } else {
    group.appendChild(svgText(12, 22, CARD_ICONS[status] || '\u25cb', 'card-icon'));
  }

  var chipText = graphChipText(entry);
  var chipW = chipText.length * CARD_CHIP_CHAR + 12;
  var chipX = w - chipW - 8 - (status === 'running' ? 30 : 0);
  group.appendChild(svgRect('card-chip', chipX, 9, chipW, 17, 4));
  group.appendChild(svgText(chipX + chipW / 2, 21.5, chipText, 'card-chip-text', 'middle'));

  var revChange = node.rev_change;
  var revBadgeW = 0;
  if (revChange === 'added' || revChange === 'changed') {
    var revLabel = revChange;
    revBadgeW = revLabel.length * 5.5 + 10;
    group.appendChild(svgRect('card-rev-badge card-rev-badge-' + revChange, 8, 4, revBadgeW, 14, 3));
    group.appendChild(svgText(8 + revBadgeW / 2, 14.5, revLabel, 'card-rev-badge-text card-rev-badge-text-' + revChange, 'middle'));
  }

  var idPad = 26 + (revBadgeW > 0 ? revBadgeW + 6 : 0);
  var idText = svgText(idPad, 22, graphFitText(entry.id, chipX - idPad - 6, CARD_ID_CHAR), 'card-id');
  group.appendChild(idText);
  var timeText = graphCardTime(entry, nowMs);
  var timeW = timeText.length * CARD_TIME_CHAR;
  group.appendChild(svgText(w - 10, 45, timeText, 'card-time', 'end'));
  group.appendChild(svgText(12, 45, graphFitText(graphCardSub(entry, geometry), w - timeW - 30, CARD_SUB_CHAR), 'card-sub'));

  group.addEventListener('click', function () { selectGraphNode(entry.id); });
  group.addEventListener('keydown', function (e) {
    if (e.key === 'Enter' || e.key === ' ' || e.key === 'Spacebar') {
      e.preventDefault();
      selectGraphNode(entry.id);
    }
  });
  return group;
}

function graphDefs() {
  var defs = svgEl('defs');
  var pattern = svgEl('pattern');
  pattern.setAttribute('id', 'graph-stripe');
  pattern.setAttribute('patternUnits', 'userSpaceOnUse');
  pattern.setAttribute('width', 12);
  pattern.setAttribute('height', 12);
  pattern.setAttribute('patternTransform', 'rotate(45)');
  pattern.appendChild(svgRect('stripe', 0, 0, 6, 12));
  defs.appendChild(pattern);
  return defs;
}

function applyGraphTransform() {
  if (!graphZoomGroup) { return; }
  graphZoomGroup.setAttribute(
    'transform',
    'translate(' + graphTransform.x + ',' + graphTransform.y + ') scale(' + graphTransform.scale + ')'
  );
}

// Key the timeline by ids, lanes and sub-rows, so a pan/zoom is kept across
// status ticks and only reset when the layout itself changes.
function graphGeometryKey(geometry) {
  var parts = [];
  for (var i = 0; i < geometry.entries.length; i++) {
    var entry = geometry.entries[i];
    parts.push(entry.id + '@' + entry.lane + ',' + entry.subrow);
  }
  return parts.join('|');
}

// UX-934: rebuild swimlane cards when revision marks change but layout ids stay put.
function graphNodesRevKey(graph) {
  var nodes = graph && Array.isArray(graph.nodes) ? graph.nodes : [];
  var parts = [];
  for (var i = 0; i < nodes.length; i++) {
    var node = nodes[i];
    parts.push(String(node.id) + ':' + String(node.rev_change || ''));
  }
  return parts.join('|');
}

// The canvas grows with the lanes but stays within [180px, 70vh].
function graphCanvasHeight(geometry) {
  var viewportMax = Math.round(window.innerHeight * GRAPH_MAX_H_RATIO);
  var maxH = Math.max(GRAPH_MIN_H, viewportMax);
  return Math.max(GRAPH_MIN_H, Math.min(geometry.height + 2, maxH));
}

// UX-703 / UX-881: Fit frames the timeline but never scales text below
// GRAPH_TEXT_FLOOR px; wider timelines pan horizontally instead. Returns false
// when the canvas has no measured size (e.g. while the List view hides it).
function autoFitGraph(geometry) {
  var canvas = $('graph-canvas');
  if (!canvas) { return false; }
  var cw = canvas.clientWidth;
  var ch = canvas.clientHeight;
  if (!cw || !ch) { return false; }
  var floor = GRAPH_TEXT_FLOOR / GRAPH_MIN_FONT;
  var scale = Math.min(cw / geometry.width, ch / geometry.height, 1);
  scale = Math.max(floor, Math.max(GRAPH_MIN_SCALE, Math.min(GRAPH_MAX_SCALE, scale)));
  graphTransform.scale = scale;
  graphTransform.x = 0;
  graphTransform.y = 0;
  return true;
}

function renderGraphCanvas(graph, nowMs, geometryOpt, geometryKeyOpt) {
  var canvas = $('graph-canvas');
  if (!canvas) { return; }
  var active = document.activeElement;
  var focusId = active && active.closest && active.closest('.graph-svg g.node[data-id]')
    ? active.closest('.graph-svg g.node[data-id]').getAttribute('data-id')
    : null;
  graphZoomGroup = null;
  canvas.replaceChildren();
  var nodes = graph && Array.isArray(graph.nodes) ? graph.nodes : [];
  if (nodes.length === 0) {
    canvas.appendChild(el('p', 'empty', 'no nodes in this run'));
    return;
  }

  var geometry = geometryOpt ||
    buildSwimlaneGeometry(graph, graphServerNow(snapshot, nowMs), graphTimelineWidth());
  graphGeometry = geometry;
  graphLastBuildAt = Date.now();
  var width = geometry.width;
  var height = geometry.height;
  canvas.style.height = graphCanvasHeight(geometry) + 'px';
  var svg = svgEl('svg');
  svg.setAttribute('class', 'graph-svg swimlane');
  // The svg fills the canvas; the zoom group carries the pan/zoom transform,
  // so panning never clips the timeline at the svg edge.
  var svgW = Math.max(width, canvas.clientWidth || 0);
  var svgH = Math.max(height, graphCanvasHeight(geometry));
  svg.setAttribute('width', svgW);
  svg.setAttribute('height', svgH);
  svg.setAttribute('viewBox', '0 0 ' + svgW + ' ' + svgH);
  svg.setAttribute('role', 'img');
  svg.setAttribute('aria-label', 'Run timeline, ' + geometry.lanes.length + ' runners, ' +
    nodes.length + ' nodes');
  svg.appendChild(graphDefs());

  var zoom = svgEl('g');
  zoom.setAttribute('class', 'graph-zoom');
  graphZoomGroup = zoom;
  var i;

  // Axis: ticks, labels and vertical grid lines (FR-880).
  var axis = svgEl('g');
  axis.setAttribute('class', 'graph-axis');
  for (var t = 0; t <= geometry.spanDraw + 1; t += geometry.tickStep) {
    var tx = LANE_LABEL_W + t * geometry.pxPerMs;
    axis.appendChild(svgLine('axis-grid', tx, AXIS_H, tx, height));
    axis.appendChild(svgText(tx, AXIS_H - 10, fmtTick(t), 'axis-label', t === 0 ? 'start' : 'middle'));
  }
  axis.appendChild(svgLine('axis-base', LANE_LABEL_W, AXIS_H, width, AXIS_H));
  zoom.appendChild(axis);

  // FR-883: shade the future while the run is active.
  if (!geometry.ended) {
    zoom.appendChild(svgRect('graph-future', geometry.nowX, AXIS_H, Math.max(0, width - geometry.nowX), height - AXIS_H));
  }

  // Lanes: label column (runner + latest model) and separators (FR-880).
  for (i = 0; i < geometry.lanes.length; i++) {
    var lane = geometry.lanes[i];
    var laneG = svgEl('g');
    laneG.setAttribute('class', 'lane lane-c' + (lane.index % GRAPH_LANE_COLORS));
    laneG.setAttribute('data-runner', lane.runner);
    laneG.appendChild(svgRect('lane-label', 0, lane.top, LANE_LABEL_W, lane.height));
    laneG.appendChild(svgRect('lane-accent', 0, lane.top, 4, lane.height));
    var mid = lane.top + lane.height / 2;
    laneG.appendChild(svgText(16, mid - 3, graphFitText(runnerLaneName(lane.runner), 145, 8), 'lane-name'));
    laneG.appendChild(svgText(16, mid + 15, graphFitText(lane.model, 145, 6.4), 'lane-model'));
    laneG.appendChild(svgLine('lane-sep', 0, lane.top + lane.height, width, lane.top + lane.height));
    zoom.appendChild(laneG);
  }

  // FR-884: dependency arrows, colored (and dashed) by the source state.
  var depsMap = graphDepsMap(graph);
  for (var targetId in depsMap) {
    if (!Object.prototype.hasOwnProperty.call(depsMap, targetId)) { continue; }
    var to = geometry.positions[targetId];
    if (!to) { continue; }
    var deps = depsMap[targetId];
    var headStates = [];
    for (var d = 0; d < deps.length; d++) {
      var from = geometry.positions[deps[d]];
      if (!from || from.id === to.id) { continue; }
      var state = graphNodeStatus(from.node);
      var path = svgEl('path');
      path.setAttribute('class', 'edge edge-' + state);
      path.setAttribute('data-from', from.id);
      path.setAttribute('data-to', to.id);
      path.setAttribute('d', graphArrowPath(from, to, geometry.entries));
      zoom.appendChild(path);
      headStates.push(state);
    }
    // One head per target (TC-884): all incoming arrows end at the same point.
    if (headStates.length) {
      var head = graphArrowHead(to, 'edge-head edge-head-' + graphHeadState(headStates));
      head.setAttribute('data-to', to.id);
      zoom.appendChild(head);
    }
  }

  for (i = 0; i < geometry.entries.length; i++) {
    zoom.appendChild(graphCard(geometry.entries[i], geometry, nowMs));
  }

  // FR-883: the now (or ended) line on top, with its label in the axis band.
  var nowG = svgEl('g');
  nowG.setAttribute('class', 'now-line' + (geometry.ended ? ' now-ended' : ''));
  nowG.appendChild(svgLine('now-rule', geometry.nowX, 4, geometry.nowX, height));
  var labelW = geometry.nowLabel.length * 6.6 + 12;
  var labelX = Math.max(0, Math.min(geometry.nowX, width - labelW - 2));
  nowG.appendChild(svgRect('now-tag', labelX, 4, labelW, 18, 4));
  nowG.appendChild(svgText(labelX + 6, 17, geometry.nowLabel, 'now-text'));
  zoom.appendChild(nowG);

  var key = geometryKeyOpt || graphGeometryKey(geometry);
  if (!graphUserInteracted) {
    autoFitGraph(geometry);
  }
  lastGraphGeometryKey = key;

  svg.appendChild(zoom);
  canvas.appendChild(svg);
  applyGraphTransform();
  if (focusId) {
    var again = graphNodeElementById(focusId);
    if (again) { again.focus(); }
  }
}

function updateGraphNodeSelection() {
  var groups = document.querySelectorAll('.graph-svg g.node[data-id]');
  for (var i = 0; i < groups.length; i++) {
    var id = groups[i].getAttribute('data-id');
    groups[i].classList.toggle('node-selected', id === graphSelectedId);
    groups[i].classList.toggle('node-task-highlight', !!taskChecklistHighlight[id]);
  }
}

// Called every tick while something runs: refresh the running cards' elapsed
// text, and rebuild the timeline every GRAPH_REBUILD_MS so the now line and
// running tails move (never while the pointer is down, so clicks land).
function updateRunningGraphCards(graph, nowMs) {
  if (!graph || !graphGeometry) { return; }
  if (Date.now() - graphLastBuildAt >= GRAPH_REBUILD_MS && !graphPointerDown && !graphPan) {
    renderGraphCanvas(graph, nowMs);
    updateGraphNodeSelection();
    return;
  }
  var nodes = Array.isArray(graph.nodes) ? graph.nodes : [];
  for (var i = 0; i < nodes.length; i++) {
    var node = nodes[i];
    if (graphNodeStatus(node) !== 'running') { continue; }
    var entry = graphGeometry.positions[String(node.id)];
    var group = graphNodeElementById(String(node.id));
    if (!entry || !group) { continue; }
    var time = group.querySelector('.card-time');
    if (time) { time.textContent = graphCardTime(entry, nowMs); }
  }
}

function syncGraphCanvas(graph, nowMs, revision) {
  if (!graph) { return; }
  var geometry = buildSwimlaneGeometry(graph, graphServerNow(snapshot, nowMs), graphTimelineWidth());
  var key = graphGeometryKey(geometry);
  var nodesKey = graphNodesRevKey(graph);
  var needsBuild = lastGraphCanvasRevision !== revision ||
    lastGraphCanvasGeometryKey !== key ||
    lastGraphCanvasNodesKey !== nodesKey ||
    !document.querySelector('.graph-svg');
  if (needsBuild) {
    renderGraphCanvas(graph, nowMs, geometry, key);
    lastGraphCanvasRevision = revision;
    lastGraphCanvasGeometryKey = key;
    lastGraphCanvasNodesKey = nodesKey;
    updateGraphNodeSelection();
    return;
  }
  updateRunningGraphCards(graph, nowMs);
  updateGraphNodeSelection();
}

// TC-881: pure helpers for the headless-Chrome math checks.
window.dmuxaiGraph = {
  estimate: graphEstimate,
  arrowPath: graphArrowPath,
  tickStep: graphTickStep,
  headState: graphHeadState,
  fitText: graphFitText,
  layout: buildSwimlaneGeometry,
  contrast: contrastRatio
};

function selectGraphNode(id) {
  graphSelectedId = id;
  var graph = snapshot && snapshot.graph ? snapshot.graph : null;
  var now = lastRendered || Date.now();
  updateGraphNodeSelection();
  renderGraphDetail(graph, now);
}

function closeGraphDetail() {
  var focusId = graphSelectedId;
  graphSelectedId = null;
  var panel = $('graph-detail');
  if (panel) {
    panel.hidden = true;
    panel.replaceChildren();
  }
  updateGraphNodeSelection();
  if (focusId) {
    var node = graphNodeElementById(focusId);
    if (node) { node.focus(); }
  }
}

function detailRow(list, label, value) {
  list.appendChild(el('dt', null, label));
  list.appendChild(el('dd', null, value));
}

function detailSection(title, items, className) {
  var section = el('div', 'detail-section');
  section.appendChild(el('h4', 'detail-section-title', title));
  if (!items || items.length === 0) {
    section.appendChild(el('p', 'detail-empty', 'none'));
    return section;
  }
  if (className === 'detail-log') {
    var pre = el('pre', className);
    var lines = [];
    for (var i = 0; i < items.length; i++) { lines.push(String(items[i])); }
    pre.textContent = lines.join('\n');
    section.appendChild(pre);
  } else {
    var ul = el('ul', className);
    for (var j = 0; j < items.length; j++) {
      ul.appendChild(el('li', null, String(items[j])));
    }
    section.appendChild(ul);
  }
  return section;
}

function renderGraphDetail(graph, nowMs) {
  var panel = $('graph-detail');
  if (!panel) { return; }
  var nodes = graph && Array.isArray(graph.nodes) ? graph.nodes : [];
  var node = null;
  for (var i = 0; i < nodes.length; i++) {
    if (String(nodes[i].id) === graphSelectedId) { node = nodes[i]; break; }
  }
  if (!node || graphViewMode !== 'graph') {
    if (!node) { graphSelectedId = null; }
    panel.hidden = true;
    panel.replaceChildren();
    return;
  }

  panel.hidden = false;
  panel.replaceChildren();

  var head = el('div', 'detail-head');
  head.appendChild(el('h3', 'detail-title', String(node.id)));
  var close = document.createElement('button');
  close.type = 'button';
  close.className = 'btn detail-close';
  close.textContent = 'Close';
  close.setAttribute('aria-label', 'Close node details');
  close.addEventListener('click', closeGraphDetail);
  head.appendChild(close);
  panel.appendChild(head);

  var status = graphNodeStatus(node);
  panel.appendChild(el('span', 'badge badge-' + status, status));

  var list = el('dl', 'detail-grid');
  detailRow(list, 'Runner', runnerShortName(node.runner) || '\u2013');
  detailRow(list, 'Model', String(node.model_id || '\u2013'));
  detailRow(list, 'Role', String(node.role || '\u2013'));
  detailRow(list, 'Attempt', String(typeof node.attempt === 'number' ? node.attempt : 0));
  var ms = nodeElapsed(node, snapshot, nowMs);
  detailRow(list, 'Elapsed', ms === null ? '\u2013' : fmtElapsed(ms));
  detailRow(list, 'Job', String(node.job || '\u2013'));
  panel.appendChild(list);

  var detail = node.detail && typeof node.detail === 'object' ? node.detail : {};
  panel.appendChild(detailSection(
    'Log tail', Array.isArray(detail.log_tail) ? detail.log_tail : [], 'detail-log'
  ));
  panel.appendChild(detailSection(
    'Changed files', Array.isArray(detail.changed_files) ? detail.changed_files : [], 'detail-files'
  ));
}

// ETA is only derivable from real timings: at least one finished node with a
// measured duration, and work still remaining. Otherwise it is omitted.
function estimateEtaMs(graph) {
  var nodes = graph && Array.isArray(graph.nodes) ? graph.nodes : [];
  var total = 0;
  var done = 0;
  var active = 0;
  var sum = 0;
  var finished = 0;
  for (var i = 0; i < nodes.length; i++) {
    var node = nodes[i];
    total += 1;
    var status = graphNodeStatus(node);
    if (status === 'completed' || status === 'failed' || status === 'skipped') {
      done += 1;
      var ms = null;
      if (typeof node.elapsed_ms === 'number') {
        ms = node.elapsed_ms;
      } else if (
        typeof node.started_at_ms === 'number' && typeof node.finished_at_ms === 'number'
      ) {
        ms = node.finished_at_ms - node.started_at_ms;
      }
      if (ms !== null && ms > 0) { sum += ms; finished += 1; }
    } else if (status === 'running') {
      active += 1;
    }
  }
  var remaining = total - done;
  if (remaining <= 0 || finished === 0) { return null; }
  var concurrency = active > 0 ? active : 1;
  return Math.round((sum / finished) * remaining / concurrency);
}

function renderGraphSummary(graph) {
  renderBar(graph);
  renderCounts(graph);

  var counts = graphCounts(graph);
  var total = 0;
  var completed = 0;
  var i;
  for (i = 0; i < counts.length; i++) {
    total += counts[i].n;
    if (counts[i].key === 'completed') { completed = counts[i].n; }
  }

  var percentEl = $('graph-percent');
  if (percentEl) {
    percentEl.textContent = total > 0 ? Math.round((completed / total) * 100) + '%' : '0%';
  }

  var etaEl = $('graph-eta');
  if (etaEl) {
    var etaMs = estimateEtaMs(graph);
    if (etaMs === null) {
      etaEl.hidden = true;
      etaEl.textContent = '';
    } else {
      etaEl.hidden = false;
      etaEl.textContent = 'ETA ~' + fmtElapsed(etaMs);
    }
  }
}

function resolveGraphViewMode(graph) {
  if (graphViewPref === 'graph' || graphViewPref === 'list') { return graphViewPref; }
  var layers = graph && typeof graph.layers === 'number' ? graph.layers : 0;
  return layers > 1 ? 'graph' : 'list';
}

function applyGraphViewMode() {
  var graph = snapshot && snapshot.graph ? snapshot.graph : null;
  graphViewMode = resolveGraphViewMode(graph);

  var list = $('nodes');
  var stage = $('graph-stage');
  var detail = $('graph-detail');
  var listBtn = $('view-list');
  var graphBtn = $('view-graph');

  if (list) { list.hidden = graphViewMode !== 'list'; }
  if (stage) { stage.hidden = graphViewMode !== 'graph'; }
  if (detail && (graphViewMode !== 'graph' || !graphSelectedId)) { detail.hidden = true; }

  if (listBtn) {
    listBtn.classList.toggle('on', graphViewMode === 'list');
    listBtn.setAttribute('aria-pressed', graphViewMode === 'list' ? 'true' : 'false');
  }
  if (graphBtn) {
    graphBtn.classList.toggle('on', graphViewMode === 'graph');
    graphBtn.setAttribute('aria-pressed', graphViewMode === 'graph' ? 'true' : 'false');
  }
}

function setGraphViewChoice(mode) {
  graphViewPref = mode === 'list' ? 'list' : 'graph';
  try { window.localStorage.setItem(GRAPH_VIEW_KEY, graphViewPref); } catch (err) { /* ignore */ }
  if (graphViewPref === 'list') {
    closeGraphDetail();
  }
  applyGraphViewMode();
  if (graphViewPref === 'graph') {
    var graph = snapshot && snapshot.graph ? snapshot.graph : null;
    var now = lastRendered || Date.now();
    var rev = snapshot && typeof snapshot.revision === 'number' ? snapshot.revision : null;
    if (graph && rev !== null) {
      syncGraphCanvas(graph, now, rev);
    }
    renderGraphDetail(graph, now);
  }
}

function onGraphWheel(e) {
  if (!graphZoomGroup) { return; }
  e.preventDefault();
  var canvas = $('graph-canvas');
  if (!canvas) { return; }
  graphUserInteracted = true;
  var rect = canvas.getBoundingClientRect();
  var px = e.clientX - rect.left;
  var py = e.clientY - rect.top;
  var factor = e.deltaY < 0 ? 1.1 : 1 / 1.1;
  var scale = graphTransform.scale * factor;
  scale = Math.max(GRAPH_MIN_SCALE, Math.min(GRAPH_MAX_SCALE, scale));
  var ratio = scale / graphTransform.scale;
  graphTransform.x = px - (px - graphTransform.x) * ratio;
  graphTransform.y = py - (py - graphTransform.y) * ratio;
  graphTransform.scale = scale;
  applyGraphTransform();
}

function onGraphDragStart(e) {
  graphPointerDown = true;
  if (!graphZoomGroup) { return; }
  if (e.target && e.target.closest && e.target.closest('.node')) { return; }
  graphUserInteracted = true;
  graphPan = { x: e.clientX, y: e.clientY, ox: graphTransform.x, oy: graphTransform.y };
}

function onGraphDragMove(e) {
  if (!graphPan) { return; }
  graphTransform.x = graphPan.ox + (e.clientX - graphPan.x);
  graphTransform.y = graphPan.oy + (e.clientY - graphPan.y);
  applyGraphTransform();
}

function onGraphDragEnd() {
  graphPan = null;
  graphPointerDown = false;
}

// The timeline width follows the canvas width, so re-lay it out on resize.
var graphResizeTimer = null;
function onGraphResize() {
  if (graphResizeTimer !== null) { clearTimeout(graphResizeTimer); }
  graphResizeTimer = setTimeout(function () {
    graphResizeTimer = null;
    var graph = snapshot && snapshot.graph ? snapshot.graph : null;
    if (graph && graphViewMode === 'graph') {
      renderGraphCanvas(graph, lastRendered || Date.now());
      updateGraphNodeSelection();
    }
  }, 150);
}

function onGraphKeydown(e) {
  if (e.key === 'Escape' && graphSelectedId) { closeGraphDetail(); }
}

// ---------------------------------------------------------------------------
// SPEC-dashboard-fullwidth FR-1211..FR-1216: Diff and Graph are center views
// opened from header buttons. Chat stays the main center content; at most one
// view is open, it covers the center area between the docked sidebars, and
// the choice is kept per tab in sessionStorage.
// ---------------------------------------------------------------------------

var CENTER_VIEW_KEY = 'dmuxai.centerView';
var CENTER_VIEWS = { diff: { panel: 'docs', toggle: 'diff-toggle' }, graph: { panel: 'graph', toggle: 'graph-toggle' } };
var centerView = null;
var centerViewReturnFocus = null;
// Approval keys (graph id + plan rev) that already auto-opened the Graph view,
// so closing it while the plan waits does not re-open it on every snapshot.
var approvalAutoOpened = {};

function readCenterView() {
  var v = null;
  try { v = window.sessionStorage.getItem(CENTER_VIEW_KEY); } catch (err) { v = null; }
  return CENTER_VIEWS[v] ? v : null;
}

function writeCenterView(v) {
  try { window.sessionStorage.setItem(CENTER_VIEW_KEY, v || ''); } catch (err) { /* ignore */ }
}

function setCenterView(next, opts) {
  opts = opts || {};
  if (next && !CENTER_VIEWS[next]) { next = null; }
  var prev = centerView;
  centerView = next;
  Object.keys(CENTER_VIEWS).forEach(function (name) {
    var cfg = CENTER_VIEWS[name];
    var panel = $(cfg.panel);
    var toggle = $(cfg.toggle);
    var open = name === next;
    if (panel) {
      panel.hidden = !open;
      panel.classList.toggle('open', open);
    }
    if (toggle) {
      toggle.setAttribute('aria-expanded', open ? 'true' : 'false');
      toggle.classList.toggle('active', open);
    }
  });
  document.body.classList.toggle('center-view-open', !!next);
  if (opts.persist !== false) { writeCenterView(next); }
  if (next === 'diff') {
    // The Diff view is the change list; show it expanded.
    var docsToggle = $('docs-toggle');
    if (docsToggle && docsToggle.getAttribute('aria-expanded') !== 'true') { docsToggle.click(); }
  }
  if (next === 'graph') {
    // The canvas had no width while hidden; lay it out for the open view.
    onGraphResize();
  }
  if (next && next !== prev && opts.focus !== false) {
    centerViewReturnFocus = document.activeElement;
    var close = document.querySelector('#' + CENTER_VIEWS[next].panel + ' .center-view-close');
    if (close) { close.focus(); }
  } else if (!next && prev && opts.focus !== false) {
    var back = $(CENTER_VIEWS[prev].toggle);
    if (back) { back.focus(); }
  }
}

// docs.js opens the Diff view from chat document cards and closes it for
// "Show in chat".
window.dmuxaiSetCenterView = function (name) { setCenterView(name || null); };

function toggleCenterView(name) {
  setCenterView(centerView === name ? null : name);
}

function graphAwaitingApproval(graph) {
  return !!(graph && graph.phase === 'awaiting_approval' && graph.approval);
}

// Graph button indicator: "!" while a plan waits for approval, a pulsing dot
// while the run is active.
function updateGraphIndicator(snap) {
  var ind = $('graph-indicator');
  var toggle = $('graph-toggle');
  if (!ind || !toggle) { return; }
  var graph = snap ? snap.graph : null;
  var label = 'Graph';
  ind.className = 'view-indicator';
  if (graphAwaitingApproval(graph)) {
    ind.hidden = false;
    ind.textContent = '!';
    ind.classList.add('view-indicator-approval');
    label = 'Graph, plan awaiting approval';
  } else if (graph && isSomethingRunning(snap) && graph.phase !== 'completed' && graph.phase !== 'failed') {
    ind.hidden = false;
    ind.textContent = '';
    ind.classList.add('view-indicator-running');
    label = 'Graph, run active';
  } else {
    ind.hidden = true;
    ind.textContent = '';
  }
  toggle.setAttribute('aria-label', label);
}

// FR-1214, FR-1221: approval is never hidden. A new pending plan opens the
// Graph view once; the chat always shows an inline approval card (session.js)
// with Approve / Edit / Reject while the plan waits.
function syncCenterViewsWithSnapshot(snap) {
  var graph = snap ? snap.graph : null;
  updateGraphIndicator(snap);
  if (graphAwaitingApproval(graph)) {
    var key = String(graph.id || '') + ':' + String(graph.approval.rev);
    if (!approvalAutoOpened[key]) {
      approvalAutoOpened[key] = true;
      if (centerView !== 'graph') { setCenterView('graph', { focus: false }); }
    }
  }
}

// Hooks for the inline chat rows (session.js, FR-1217..FR-1221).
window.dmuxaiServerNow = function () { return graphServerNow(snapshot, Date.now()); };

window.dmuxaiFocusGraphNode = function (id) {
  setCenterView('graph', { focus: false });
  if (graphViewMode !== 'graph') { setGraphViewChoice('graph'); }
  // The canvas is laid out after the view opens; select once it is drawn.
  window.setTimeout(function () {
    selectGraphNode(id);
    var detail = $('graph-detail');
    if (detail && !detail.hidden) {
      detail.setAttribute('tabindex', '-1');
      detail.focus();
    }
  }, 250);
};

// SPEC-dashboard-fullwidth FR-1224: open the Graph view on a run of the
// current conversation. An older run is loaded by the `show_graph_run`
// command; the node is focused once its snapshot arrives. Resolves to
// {ok, error}.
var pendingRunFocus = null;

window.dmuxaiShowGraphRun = function (runId, nodeId) {
  var graph = snapshot && snapshot.graph;
  if (graph && graph.id === runId) {
    if (nodeId) { window.dmuxaiFocusGraphNode(nodeId); } else { setCenterView('graph'); }
    return Promise.resolve({ ok: true });
  }
  if (isDashboardViewOnly()) {
    return Promise.resolve({ ok: false, error: 'The read-only view shows the latest run only.' });
  }
  pendingRunFocus = { runId: runId, nodeId: nodeId || null, until: Date.now() + 10000 };
  return todoApi('POST', '/api/command', { command: 'show_graph_run', run_id: runId }).then(function () {
    poll();
    return { ok: true };
  }).catch(function (err) {
    pendingRunFocus = null;
    return { ok: false, error: err && err.message ? err.message : 'Could not open that run.' };
  });
};

function applyPendingRunFocus(snap) {
  if (!pendingRunFocus) { return; }
  if (Date.now() > pendingRunFocus.until) { pendingRunFocus = null; return; }
  var graph = snap && snap.graph;
  if (!graph || graph.id !== pendingRunFocus.runId) { return; }
  var nodeId = pendingRunFocus.nodeId;
  pendingRunFocus = null;
  if (nodeId) { window.dmuxaiFocusGraphNode(nodeId); } else { setCenterView('graph'); }
}

// Chat approval card hooks (FR-1221); kept apart from window.dmuxaiPlanGate.
window.dmuxaiChatPlanGate = {
  disabled: function () { return planGateDisabled() || !currentPlanGateContext(); },
  action: function (kind) {
    var g = snapshot && snapshot.graph;
    if (kind === 'open') {
      setCenterView('graph');
      var panel = $('graph-approval');
      if (panel && panel.scrollIntoView) { panel.scrollIntoView({ block: 'nearest' }); }
      return;
    }
    if (planGateDisabled() || !g) { return; }
    if (kind === 'approve') { submitPlanDecision('approve'); return; }
    if (kind === 'reject') {
      if (!window.confirm('Reject this plan revision?')) { return; }
      submitPlanDecision('reject');
      return;
    }
    if (kind === 'edit') {
      // Edit needs the feedback box, which lives in the Graph view.
      setCenterView('graph', { focus: false });
      planGateUi.editOpen = true;
      approvalPanelFocusFeedback = true;
      renderApprovalPanel(g);
    }
  }
};

function bindCenterViews() {
  Object.keys(CENTER_VIEWS).forEach(function (name) {
    var toggle = $(CENTER_VIEWS[name].toggle);
    if (toggle) { toggle.addEventListener('click', function () { toggleCenterView(name); }); }
  });
  var closes = document.querySelectorAll('.center-view-close');
  for (var i = 0; i < closes.length; i++) {
    closes[i].addEventListener('click', function () { setCenterView(null); });
  }
  // Capture phase: runs before the inner layers' own Esc handlers, so it
  // sees their state before they close.
  window.addEventListener('keydown', function (e) {
    if (e.key !== 'Escape' || !centerView || e.defaultPrevented) { return; }
    // Inner layers close first: the document preview, node details, the
    // command palette and the plan edit box keep their own Esc.
    var preview = document.querySelector('.doc-overlay');
    if (preview && !preview.hidden) { return; }
    if (centerView === 'graph' && graphSelectedId) { return; }
    if (document.querySelector('.session-overlay')) { return; }
    var t = e.target;
    if (t && t.classList && t.classList.contains('graph-approval-feedback')) { return; }
    if (t && t.closest && t.closest('.sidebar, .sessions-sidebar, .workspace-dialog-backdrop')) { return; }
    setCenterView(null);
  }, true);
  setCenterView(readCenterView(), { persist: false, focus: false });
}

function bindGraphControls() {
  var listBtn = $('view-list');
  var graphBtn = $('view-graph');
  var fit = $('graph-fit');
  if (listBtn) { listBtn.addEventListener('click', function () { setGraphViewChoice('list'); }); }
  if (graphBtn) { graphBtn.addEventListener('click', function () { setGraphViewChoice('graph'); }); }
  var replanBtn = $('graph-replan');
  if (replanBtn) {
    replanBtn.addEventListener('click', function () { submitPlanReplan(); });
  }
  var historyBtn = $('plan-history');
  if (historyBtn) {
    historyBtn.addEventListener('click', function (e) {
      e.stopPropagation();
      if (planGateDisabled()) { return; }
      planHistoryUi.open = !planHistoryUi.open;
      var graph = snapshot && snapshot.graph;
      if (graph) { renderPlanHistoryPanel(graph); }
    });
  }
  document.addEventListener('click', function (e) {
    if (!planHistoryUi.open) { return; }
    var wrap = document.querySelector('.graph-history-wrap');
    if (wrap && e.target instanceof Node && wrap.contains(e.target)) { return; }
    planHistoryUi.open = false;
    var graph = snapshot && snapshot.graph;
    if (graph) { renderPlanHistoryPanel(graph); }
  });
  if (fit) {
    fit.addEventListener('click', function () {
      // Re-enable auto-fit and frame the whole map again.
      graphUserInteracted = false;
      if (graphGeometry && autoFitGraph(graphGeometry)) {
        applyGraphTransform();
        return;
      }
      var graph = snapshot && snapshot.graph ? snapshot.graph : null;
      if (graph) { renderGraphCanvas(graph, lastRendered || Date.now()); }
    });
  }
  var canvas = $('graph-canvas');
  if (canvas) {
    canvas.addEventListener('wheel', onGraphWheel, { passive: false });
    canvas.addEventListener('mousedown', onGraphDragStart);
    canvas.addEventListener('mousemove', onGraphDragMove);
    canvas.addEventListener('mouseup', onGraphDragEnd);
    canvas.addEventListener('mouseleave', onGraphDragEnd);
  }
  window.addEventListener('resize', onGraphResize);
  // SPEC-dashboard-fullwidth FR-1201: the center column now follows the
  // docked sidebars, so a sidebar toggle or drag changes the canvas width
  // without a window resize. Re-lay the timeline on width changes only (the
  // render itself sets the canvas height).
  if (canvas && typeof window.ResizeObserver === 'function') {
    var lastCanvasWidth = canvas.clientWidth;
    new window.ResizeObserver(function () {
      var w = canvas.clientWidth;
      if (w && w !== lastCanvasWidth) {
        lastCanvasWidth = w;
        onGraphResize();
      }
    }).observe(canvas);
  }
  document.addEventListener('keydown', onGraphKeydown);

  var stored = null;
  try { stored = window.localStorage.getItem(GRAPH_VIEW_KEY); } catch (err) { stored = null; }
  graphViewPref = (stored === 'graph' || stored === 'list') ? stored : null;
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

// UX-930 / TC-957: plain-text judge line from snapshot `graph.triage.line`.
function fillJudgeLineContent(container, text, highlightFallback) {
  container.replaceChildren();
  if (!text) { return; }
  if (!highlightFallback || text.indexOf('fallback') < 0) {
    container.appendChild(document.createTextNode(text));
    return;
  }
  var token = 'fallback';
  var rest = text;
  while (rest.length > 0) {
    var index = rest.indexOf(token);
    if (index < 0) {
      container.appendChild(document.createTextNode(rest));
      break;
    }
    if (index > 0) {
      container.appendChild(document.createTextNode(rest.slice(0, index)));
    }
    container.appendChild(el('span', 'judge-fallback-word', token));
    rest = rest.slice(index + token.length);
  }
}

function renderGraphJudgeLine(graph) {
  var lineEl = $('graph-judge-line');
  if (!lineEl) { return; }
  var triage = graph && graph.triage;
  var text = triage && typeof triage.line === 'string' ? triage.line : '';
  if (!text) {
    lineEl.hidden = true;
    lineEl.replaceChildren();
    lineEl.removeAttribute('title');
    return;
  }
  lineEl.hidden = false;
  lineEl.title = text;
  var highlightFallback = triage && triage.source === 'fallback';
  fillJudgeLineContent(lineEl, text, highlightFallback);
}

// UX-932 / FR-965: plan approval panel and task edits at the gate.
var PLAN_REV_MAX = 5;
var planGateUi = {
  busy: false,
  editOpen: false,
  editDraft: '',
  error: ''
};
var planGateContextKey = '';
var approvalPanelLastKey = null;
var approvalPanelFocusFeedback = false;

function isDashboardViewOnly() {
  var snap = snapshot;
  return !!(snap && String(snap.source || '') === 'files');
}

function planGateDisabled() {
  return isDashboardViewOnly() || planGateUi.busy;
}

function syncSessionPlanGateButtons() {
  var disabled = planGateDisabled();
  ['plan-approve', 'plan-edit', 'plan-reject', 'plan-replan'].forEach(function (id) {
    var btn = document.getElementById(id);
    if (btn) { btn.disabled = disabled; }
  });
  var graph = snapshot && snapshot.graph;
  var replanBtn = $('graph-replan');
  if (replanBtn) {
    replanBtn.disabled = disabled || !graphReplanAllowed(graph);
  }
  var historyBtn = $('plan-history');
  if (historyBtn && !historyBtn.hidden) {
    historyBtn.disabled = false;
  }
}

function resetPlanGateUiIfContextChanged(graph) {
  var rev = graph && graph.approval ? String(graph.approval.rev) : '';
  var next = graph ? String(graph.id || '') + ':' + rev : '';
  if (next === planGateContextKey) { return; }
  planGateContextKey = next;
  planGateUi.error = '';
  planGateUi.editOpen = false;
  planGateUi.editDraft = '';
  approvalPanelLastKey = null;
}

function approvalPanelRenderKey(graph, approval) {
  return JSON.stringify({
    rev: approval.rev,
    source: approval.source,
    fallback: approval.fallback,
    steps: approval.steps,
    notes: approval.notes,
    tasks: graph.tasks,
    error: planGateUi.error,
    busy: planGateUi.busy,
    viewOnly: isDashboardViewOnly(),
    editOpen: planGateUi.editOpen
  });
}

function syncApprovalPanelButtonStates() {
  var disabled = planGateDisabled();
  var panel = $('graph-approval');
  if (panel && !panel.hidden) {
    // Only plan-gate controls: buttons with their own rules (move ends, history) are left alone.
    var buttons = panel.querySelectorAll('button[data-plan-gate]');
    for (var b = 0; b < buttons.length; b++) {
      buttons[b].disabled = disabled;
    }
    var feedback = panel.querySelector('.graph-approval-feedback');
    if (feedback) { feedback.disabled = disabled; }
  }
  ['plan-approve', 'plan-edit', 'plan-reject', 'plan-replan'].forEach(function (id) {
    var btn = document.getElementById(id);
    if (btn) { btn.disabled = disabled; }
  });
  var graph = snapshot && snapshot.graph;
  var replanBtn = $('graph-replan');
  if (replanBtn) {
    replanBtn.disabled = disabled || !graphReplanAllowed(graph);
  }
  var historyBtn = $('plan-history');
  if (historyBtn && !historyBtn.hidden) {
    historyBtn.disabled = false;
  }
}

function currentPlanGateContext() {
  var graph = snapshot && snapshot.graph;
  if (!graph || graph.phase !== 'awaiting_approval' || !graph.approval) { return null; }
  return { graph: graph, approval: graph.approval };
}

function runPlanCommand(body) {
  if (isDashboardViewOnly()) { return Promise.resolve(null); }
  planGateUi.busy = true;
  planGateUi.error = '';
  syncSessionPlanGateButtons();
  var ctx = currentPlanGateContext();
  if (ctx) { renderApprovalPanel(ctx.graph); }
  return todoApi('POST', '/api/command', body).then(function (data) {
    planGateUi.busy = false;
    planGateUi.editOpen = false;
    planGateUi.editDraft = '';
    syncSessionPlanGateButtons();
    poll();
    return data;
  }).catch(function (err) {
    planGateUi.busy = false;
    planGateUi.error = err && err.message ? err.message : 'Request failed';
    syncSessionPlanGateButtons();
    var graph = snapshot && snapshot.graph;
    if (graph) { renderApprovalPanel(graph); }
    return null;
  });
}

function submitPlanDecision(decision, feedback) {
  var ctx = currentPlanGateContext();
  if (!ctx || planGateDisabled()) { return; }
  var body = {
    command: 'plan_decision',
    plan_id: String(ctx.graph.id || ''),
    revision: ctx.approval.rev,
    decision: decision
  };
  if (feedback !== undefined && feedback !== null && String(feedback).trim()) {
    body.feedback = String(feedback);
  }
  runPlanCommand(body);
}

function submitPlanTaskEdit(payload) {
  if (planGateDisabled()) { return; }
  var body = { command: 'plan_task_edit' };
  Object.keys(payload).forEach(function (key) { body[key] = payload[key]; });
  runPlanCommand(body);
}

function approvalStepMarker(revChange) {
  if (revChange === 'added') { return '+'; }
  if (revChange === 'changed') { return '~'; }
  return '';
}

function approvalDepsSuffix(dependsOn) {
  if (!dependsOn || !dependsOn.length) { return ''; }
  return ' \u2190 ' + dependsOn.join(', ');
}

function approvalStepsForTask(steps, taskId) {
  var out = [];
  for (var i = 0; i < steps.length; i++) {
    if (steps[i].task === taskId) { out.push(steps[i]); }
  }
  return out;
}

function approvalUnassignedSteps(steps, tasks) {
  var known = {};
  for (var t = 0; t < tasks.length; t++) { known[tasks[t].id] = true; }
  var out = [];
  for (var i = 0; i < steps.length; i++) {
    var task = steps[i].task;
    if (!task || !known[task]) { out.push(steps[i]); }
  }
  return out;
}

function appendApprovalStep(panel, step) {
  var stepRow = el('div', 'graph-approval-step');
  var mark = approvalStepMarker(step.rev_change);
  if (mark) {
    stepRow.appendChild(el('span', 'graph-approval-step-mark graph-approval-step-mark-' + step.rev_change, mark));
    stepRow.appendChild(document.createTextNode(' '));
  }
  var model = step.model ? String(step.model) : '';
  stepRow.appendChild(document.createTextNode(
    String(step.id || '') + ' ' +
    String(step.role || '') + ' ' +
    String(step.runner || '') + ':' + model +
    approvalDepsSuffix(step.depends_on)
  ));
  panel.appendChild(stepRow);
  var job = step.job ? String(step.job) : '';
  if (job) {
    panel.appendChild(el('div', 'graph-approval-job', job));
  }
}

function planGateActionButton(label, extraClass, onClick) {
  var btn = el('button', 'btn' + (extraClass ? ' ' + extraClass : ''), label);
  btn.type = 'button';
  btn.setAttribute('data-plan-gate', '1');
  btn.disabled = planGateDisabled();
  btn.addEventListener('click', onClick);
  return btn;
}

function planGateParityButton(label, parity, extraClass, onClick) {
  var make = window.dmuxaiSession && window.dmuxaiSession.parityButton;
  var btn;
  if (make) {
    btn = make(label, parity, onClick, extraClass);
  } else {
    btn = el('button', 'btn', label);
    btn.type = 'button';
    if (parity) { btn.setAttribute('data-parity', parity); }
    btn.addEventListener('click', onClick);
  }
  if (extraClass) {
    extraClass.split(/\s+/).forEach(function (cls) {
      if (cls) { btn.classList.add(cls); }
    });
  }
  if (parity) { btn.id = parity; }
  if (parity !== 'plan-history') { btn.setAttribute('data-plan-gate', '1'); }
  btn.disabled = planGateDisabled();
  return btn;
}

function appendTaskActions(panel, task, tasks) {
  var taskId = String(task.id || '');
  var row = el('div', 'graph-approval-task-actions');
  var idx = -1;
  for (var i = 0; i < tasks.length; i++) {
    if (tasks[i].id === taskId) { idx = i; break; }
  }
  row.appendChild(planGateActionButton('Rename', '', function () {
    var title = window.prompt('Rename task ' + taskId + ':', String(task.title || ''));
    if (title === null) { return; }
    var trimmed = String(title).trim();
    if (!trimmed) { return; }
    submitPlanTaskEdit({ action: 'rename', task_id: taskId, title: trimmed });
  }));
  row.appendChild(planGateActionButton('Split', '', function () {
    var note = window.prompt('Split ' + taskId + ' — note for the planner:');
    if (note === null) { return; }
    var trimmed = String(note).trim();
    if (!trimmed) { return; }
    submitPlanTaskEdit({ action: 'split', task_id: taskId, note: trimmed });
  }));
  row.appendChild(planGateActionButton('Merge', '', function () {
    var from = window.prompt('Merge into ' + taskId + ' — source task id:');
    if (from === null) { return; }
    var trimmed = String(from).trim();
    if (!trimmed) { return; }
    submitPlanTaskEdit({ action: 'merge', task_id: taskId, from: trimmed });
  }));
  row.appendChild(planGateActionButton('Delete', 'btn-danger', function () {
    if (!window.confirm('Delete task ' + taskId + '?')) { return; }
    submitPlanTaskEdit({ action: 'delete', task_id: taskId });
  }));
  if (idx > 0) {
    row.appendChild(planGateActionButton('Move up', '', function () {
      // Server `parse_graph_task_edit` move uses 1-based target slot (see apply_task_move).
      submitPlanTaskEdit({ action: 'move', task_id: taskId, position: idx });
    }));
  }
  if (idx >= 0 && idx < tasks.length - 1) {
    row.appendChild(planGateActionButton('Move down', '', function () {
      submitPlanTaskEdit({ action: 'move', task_id: taskId, position: idx + 2 });
    }));
  }
  panel.appendChild(row);
}

function renderApprovalPanel(graph) {
  var panel = $('graph-approval');
  if (!panel) { return; }
  resetPlanGateUiIfContextChanged(graph);

  if (!graph || graph.phase !== 'awaiting_approval' || !graph.approval) {
    panel.hidden = true;
    panel.replaceChildren();
    approvalPanelLastKey = null;
    syncSessionPlanGateButtons();
    return;
  }
  var approval = graph.approval;
  var renderKey = approvalPanelRenderKey(graph, approval);
  if (renderKey === approvalPanelLastKey) {
    syncApprovalPanelButtonStates();
    syncSessionPlanGateButtons();
    return;
  }
  approvalPanelLastKey = renderKey;

  var draft = planGateUi.editDraft;
  var editOpen = planGateUi.editOpen;
  var active = document.activeElement;
  var savedCaret = null;
  if (editOpen && active && active.classList && active.classList.contains('graph-approval-feedback')) {
    savedCaret = { start: active.selectionStart, end: active.selectionEnd };
  }

  var steps = Array.isArray(approval.steps) ? approval.steps : [];
  var tasks = Array.isArray(graph.tasks) ? graph.tasks : [];

  panel.hidden = false;
  panel.replaceChildren();

  if (planGateUi.error) {
    var errEl = el('p', 'graph-approval-error', planGateUi.error);
    errEl.setAttribute('role', 'alert');
    panel.appendChild(errEl);
  }

  panel.appendChild(el(
    'p',
    'graph-approval-header',
    'Plan rev ' + String(approval.rev) + '/' + PLAN_REV_MAX + ' \u00b7 ' + String(approval.source || '')
  ));

  if (approval.fallback) {
    panel.appendChild(el('p', 'graph-approval-warning', String(approval.fallback)));
  }

  if (!tasks.length) {
    for (var s = 0; s < steps.length; s++) {
      appendApprovalStep(panel, steps[s]);
    }
  } else {
    for (var t = 0; t < tasks.length; t++) {
      var task = tasks[t];
      var taskRow = el('p', 'graph-approval-task', String(task.id || '') + ' ' + String(task.title || ''));
      if (task.nodes && task.nodes.length) {
        taskRow.title = 'nodes: ' + task.nodes.join(', ');
      }
      panel.appendChild(taskRow);
      var taskSteps = approvalStepsForTask(steps, task.id);
      for (var ts = 0; ts < taskSteps.length; ts++) {
        appendApprovalStep(panel, taskSteps[ts]);
      }
      appendTaskActions(panel, task, tasks);
    }
    var loose = approvalUnassignedSteps(steps, tasks);
    for (var u = 0; u < loose.length; u++) {
      appendApprovalStep(panel, loose[u]);
    }
  }

  var notes = Array.isArray(approval.notes) ? approval.notes : [];
  for (var n = 0; n < notes.length; n++) {
    panel.appendChild(el('p', 'graph-approval-note', String(notes[n])));
  }

  var actions = el('div', 'graph-approval-actions');
  actions.appendChild(planGateParityButton('Approve', 'plan-approve', 'btn-primary', function () {
    submitPlanDecision('approve');
  }));
  actions.appendChild(planGateParityButton('Edit', 'plan-edit', '', function () {
    var g = snapshot && snapshot.graph;
    if (!g) { return; }
    var opening = !planGateUi.editOpen;
    planGateUi.editOpen = opening;
    if (opening) { approvalPanelFocusFeedback = true; }
    renderApprovalPanel(g);
  }));
  actions.appendChild(planGateParityButton('Reject', 'plan-reject', 'btn-danger', function () {
    if (!window.confirm('Reject this plan revision?')) { return; }
    submitPlanDecision('reject');
  }));

  if (editOpen) {
    var editWrap = el('div', 'graph-approval-edit');
    var textarea = document.createElement('textarea');
    textarea.className = 'graph-approval-feedback';
    textarea.setAttribute('aria-label', 'Plan edit feedback');
    textarea.value = draft;
    textarea.disabled = planGateDisabled();
    textarea.addEventListener('input', function () {
      planGateUi.editDraft = textarea.value;
    });
    editWrap.appendChild(textarea);
    editWrap.appendChild(planGateActionButton('Send feedback', 'btn-primary', function () {
      var text = textarea.value;
      if (!String(text).trim()) { return; }
      submitPlanDecision('edit', text);
    }));
    actions.appendChild(editWrap);
  }

  panel.appendChild(actions);
  syncSessionPlanGateButtons();

  var ta = panel.querySelector('.graph-approval-feedback');
  if (ta && savedCaret) {
    ta.focus();
    try {
      ta.setSelectionRange(savedCaret.start, savedCaret.end);
    } catch (ignore) { /* ignore */ }
  } else if (ta && approvalPanelFocusFeedback) {
    ta.focus();
  }
  approvalPanelFocusFeedback = false;
}

// UX-936: task checklist below the swimlane (`graph.tasks` from pushed snapshots).
function taskChecklistStatusClass(status) {
  var label = String(status || 'pending');
  if (label === 'running' || label === 'done' || label === 'failed') { return label; }
  return 'pending';
}

function taskChecklistStatusIcon(status) {
  var label = taskChecklistStatusClass(status);
  if (label === 'running') { return '\u25D0'; }
  if (label === 'done') { return '\u2713'; }
  if (label === 'failed') { return '\u2717'; }
  return '\u25CB';
}

function tasksForChecklist(graph) {
  if (!graph) { return []; }
  var tasks = Array.isArray(graph.tasks) ? graph.tasks : [];
  if (tasks.length) { return tasks; }
  var nodes = Array.isArray(graph.nodes) ? graph.nodes : [];
  if (nodes.length !== 1) { return []; }
  var node = nodes[0];
  var job = String(node.job || '');
  if (job.length > 80) { job = job.slice(0, 79) + '\u2026'; }
  return [{
    id: 'task-1',
    title: job,
    done_check: '',
    status: checklistStatusFromNode(node),
    nodes: [String(node.id || '')]
  }];
}

function checklistStatusFromNode(node) {
  var status = graphNodeStatus(node);
  if (status === 'running') { return 'running'; }
  if (status === 'completed' || status === 'skipped') { return 'done'; }
  if (status === 'failed') { return 'failed'; }
  return 'pending';
}

function taskChecklistNodeIdsText(nodes) {
  if (!nodes || !nodes.length) { return ''; }
  return nodes.map(function (id) { return String(id); }).join(', ');
}

function rebuildTaskChecklistHighlight(graph) {
  taskChecklistHighlight = {};
  if (!taskChecklistSelectedId || !graph) { return; }
  var tasks = tasksForChecklist(graph);
  for (var i = 0; i < tasks.length; i++) {
    var task = tasks[i];
    if (String(task.id) !== taskChecklistSelectedId) { continue; }
    var nodes = Array.isArray(task.nodes) ? task.nodes : [];
    for (var n = 0; n < nodes.length; n++) {
      taskChecklistHighlight[String(nodes[n])] = true;
    }
    return;
  }
  taskChecklistSelectedId = null;
}

function selectTaskChecklistTask(taskId) {
  var id = String(taskId || '');
  if (!id) { return; }
  if (taskChecklistSelectedId === id) {
    taskChecklistSelectedId = null;
  } else {
    taskChecklistSelectedId = id;
  }
  var graph = snapshot && snapshot.graph ? snapshot.graph : null;
  rebuildTaskChecklistHighlight(graph);
  renderTaskChecklist(graph);
  updateGraphNodeSelection();
}

function taskNodeById(graph, id) {
  var nodes = graph && Array.isArray(graph.nodes) ? graph.nodes : [];
  var want = String(id || '');
  for (var i = 0; i < nodes.length; i++) {
    if (String(nodes[i].id) === want) { return nodes[i]; }
  }
  return null;
}

function agentActivityForRunner(runner) {
  if (!snapshot || !runner) { return ''; }
  var agents = Array.isArray(snapshot.agents) ? snapshot.agents : [];
  var want = String(runner);
  for (var i = 0; i < agents.length; i++) {
    var agent = agents[i];
    if (!agent || String(agent.id) !== want) { continue; }
    if (typeof agent.activity === 'string' && agent.activity) { return agent.activity; }
  }
  return '';
}

function taskProgressLine(node, nowMs) {
  var status = graphNodeStatus(node);
  var runner = runnerShortName(node.runner) || '\u2014';
  var model = String(node.model_id || '\u2014');
  var ms = nodeElapsed(node, snapshot, nowMs);
  var elapsed = ms === null ? '\u2014' : fmtElapsed(ms);
  var job = String(node.job || '');
  if (job.length > 80) { job = job.slice(0, 79) + '\u2026'; }
  var line = String(node.id) + ' \u00b7 ' + status + ' \u00b7 ' + runner + ' ' + model + ' \u00b7 ' + elapsed;
  if (job) { line += ' \u00b7 ' + job; }
  var activity = agentActivityForRunner(node.runner);
  if (activity) { line += ' \u00b7 ' + activity; }
  return line;
}

function updateTaskChecklistRow(row, task, graph, nowMs) {
  var taskId = String(task.id || '');
  var statusClass = taskChecklistStatusClass(task.status);
  row.className = 'task-checklist-row';
  if (taskId === taskChecklistSelectedId) {
    row.classList.add('task-checklist-row-selected');
    row.classList.add('task-checklist-row-expanded');
  }
  var statusEl = row.querySelector('.task-checklist-status');
  if (!statusEl) {
    statusEl = el('span', 'task-checklist-status');
    row.insertBefore(statusEl, row.firstChild);
  }
  statusEl.className = 'task-checklist-status task-checklist-status-' + statusClass;
  statusEl.textContent = taskChecklistStatusIcon(task.status);
  var idEl = row.querySelector('.task-checklist-id');
  if (!idEl) {
    var head = el('div');
    idEl = el('span', 'task-checklist-id');
    head.appendChild(idEl);
    head.appendChild(document.createTextNode(' '));
    head.appendChild(el('span', 'task-checklist-label'));
    row.appendChild(head);
  }
  idEl.textContent = taskId;
  var labelEl = row.querySelector('.task-checklist-label');
  if (labelEl) { labelEl.textContent = String(task.title || ''); }
  var meta = row.querySelector('.task-checklist-meta');
  if (!meta) {
    meta = el('div', 'task-checklist-meta');
    meta.appendChild(el('p', 'task-checklist-done'));
    meta.appendChild(el('p', 'task-checklist-nodes'));
    row.appendChild(meta);
  }
  var doneEl = meta.querySelector('.task-checklist-done');
  if (doneEl) { doneEl.textContent = 'done: ' + String(task.done_check || ''); }
  var nodesText = taskChecklistNodeIdsText(task.nodes);
  var nodesEl = meta.querySelector('.task-checklist-nodes');
  if (nodesEl) {
    nodesEl.textContent = nodesText;
    nodesEl.hidden = !nodesText;
  }
  var promptEl = meta.querySelector('.task-checklist-prompt');
  if (!promptEl) {
    promptEl = el('p', 'task-checklist-prompt');
    meta.appendChild(promptEl);
  }
  var prompt = graph && typeof graph.user_prompt === 'string' ? graph.user_prompt : '';
  var promptText = 'prompt: ' + (prompt ? prompt : '\u2014');
  if (promptEl.textContent !== promptText) { promptEl.textContent = promptText; }
  var progress = meta.querySelector('.task-checklist-progress');
  if (!progress) {
    progress = el('div', 'task-checklist-progress');
    meta.appendChild(progress);
  }
  progress.replaceChildren();
  var nodeIds = Array.isArray(task.nodes) ? task.nodes : [];
  if (!nodeIds.length) {
    progress.appendChild(el('p', 'task-checklist-progress-line', 'progress: \u2014'));
  } else {
    for (var p = 0; p < nodeIds.length; p++) {
      var nodeId = String(nodeIds[p]);
      var node = taskNodeById(graph, nodeId);
      var text = node ? taskProgressLine(node, nowMs) : (nodeId + ' \u00b7 unknown');
      progress.appendChild(el('p', 'task-checklist-progress-line', text));
    }
  }
}

function createTaskChecklistRow(task) {
  var taskId = String(task.id || '');
  var row = el('div', 'task-checklist-row');
  row.setAttribute('role', 'button');
  row.setAttribute('tabindex', '0');
  row.setAttribute('data-task-id', taskId);
  row.addEventListener('click', function () { selectTaskChecklistTask(taskId); });
  row.addEventListener('keydown', function (e) {
    if (e.key === 'Enter' || e.key === ' ' || e.key === 'Spacebar') {
      e.preventDefault();
      selectTaskChecklistTask(taskId);
    }
  });
  updateTaskChecklistRow(row, task, null, Date.now());
  return row;
}

function renderTaskChecklist(graph) {
  var panel = $('task-checklist');
  if (!panel) { return; }
  var scrollTop = panel.scrollTop;
  var active = document.activeElement;
  var focusTaskId = active && active.classList && active.classList.contains('task-checklist-row')
    ? active.getAttribute('data-task-id') : null;
  var tasks = graph ? tasksForChecklist(graph) : [];
  if (!graph || !tasks.length) {
    panel.hidden = true;
    panel.replaceChildren();
    taskChecklistSelectedId = null;
    taskChecklistHighlight = {};
    updateGraphNodeSelection();
    return;
  }
  rebuildTaskChecklistHighlight(graph);
  panel.hidden = false;
  var titleBar = panel.querySelector('.task-checklist-title-bar');
  if (!titleBar) {
    panel.replaceChildren();
    panel.appendChild(el('p', 'task-checklist-title-bar', 'Tasks'));
  }
  var list = panel.querySelector('.task-checklist-list');
  if (!list) {
    list = el('div', 'task-checklist-list');
    panel.appendChild(list);
  }
  var existing = {};
  var rows = list.querySelectorAll('.task-checklist-row');
  for (var r = 0; r < rows.length; r++) {
    var row = rows[r];
    existing[row.getAttribute('data-task-id')] = row;
  }
  var seen = {};
  for (var i = 0; i < tasks.length; i++) {
    var task = tasks[i];
    var taskId = String(task.id || '');
    seen[taskId] = true;
    var rowNode = existing[taskId];
    if (rowNode) {
      updateTaskChecklistRow(rowNode, task, graph, Date.now());
    } else {
      rowNode = createTaskChecklistRow(task);
      updateTaskChecklistRow(rowNode, task, graph, Date.now());
    }
    if (list.children[i] !== rowNode) {
      list.insertBefore(rowNode, list.children[i] || null);
    }
  }
  for (var id in existing) {
    if (!seen[id]) { existing[id].remove(); }
  }
  panel.scrollTop = scrollTop;
  updateGraphNodeSelection();
  if (focusTaskId) {
    var focused = list.querySelector('[data-task-id="' + focusTaskId + '"]');
    if (focused) { focused.focus(); }
  }
}

// UX-934: replan + revision history in the graph header.
var planHistoryUi = { open: false };

function graphReplanAllowed(graph) {
  if (!graph) { return false; }
  var nodes = Array.isArray(graph.nodes) ? graph.nodes : [];
  if (nodes.length <= 1) { return false; }
  return String(graph.phase || '') === 'active';
}

function runPlanReplanCommand(note) {
  if (isDashboardViewOnly()) { return Promise.resolve(null); }
  planGateUi.busy = true;
  planGateUi.error = '';
  syncSessionPlanGateButtons();
  var graph = snapshot && snapshot.graph;
  if (graph) {
    renderApprovalPanel(graph);
    renderGraphHeaderActions(graph);
  }
  var body = { command: 'plan_replan' };
  if (note !== undefined && note !== null && String(note).trim()) {
    body.note = String(note).trim();
  }
  return todoApi('POST', '/api/command', body).then(function (data) {
    planGateUi.busy = false;
    syncSessionPlanGateButtons();
    poll();
    return data;
  }).catch(function (err) {
    planGateUi.busy = false;
    planGateUi.error = err && err.message ? err.message : 'Request failed';
    syncSessionPlanGateButtons();
    graph = snapshot && snapshot.graph;
    if (graph) {
      renderApprovalPanel(graph);
      renderGraphHeaderActions(graph);
    }
    return null;
  });
}

function submitPlanReplan() {
  if (planGateDisabled()) { return; }
  var graph = snapshot && snapshot.graph;
  if (!graphReplanAllowed(graph)) { return; }
  var note = window.prompt('Replan note (optional):');
  if (note === null) { return; }
  runPlanReplanCommand(note);
}

function formatPlanHistoryTime(atMs) {
  if (typeof atMs !== 'number' || !isFinite(atMs) || atMs <= 0) { return ''; }
  try {
    return new Date(atMs).toLocaleString();
  } catch (err) {
    return String(atMs);
  }
}

function planHistoryDelta(change) {
  var parts = [];
  if (change.added && change.added.length) {
    parts.push('+' + change.added.join(','));
  }
  if (change.changed && change.changed.length) {
    parts.push('~' + change.changed.join(','));
  }
  if (change.removed && change.removed.length) {
    parts.push('\u2212' + change.removed.join(','));
  }
  return parts.length ? parts.join(' ') : '\u2014';
}

function renderPlanHistoryPanel(graph) {
  var panel = $('graph-plan-history');
  var toggle = $('plan-history');
  if (!panel || !toggle) { return; }
  var scrollTop = panel.scrollTop;
  panel.replaceChildren();
  if (!planHistoryUi.open) {
    panel.hidden = true;
    toggle.setAttribute('aria-expanded', 'false');
    return;
  }
  toggle.setAttribute('aria-expanded', 'true');
  panel.hidden = false;
  var history = graph && Array.isArray(graph.plan_history) ? graph.plan_history : [];
  if (!history.length) {
    panel.appendChild(el('p', 'graph-plan-history-empty', 'No revisions yet.'));
    panel.scrollTop = scrollTop;
    return;
  }
  var start = Math.max(0, history.length - 10);
  var recent = history.slice(start).reverse();
  var list = el('ul', 'graph-plan-history-list');
  for (var i = 0; i < recent.length; i++) {
    var change = recent[i];
    var item = el('li', 'graph-plan-history-item');
    if (!change.applied) {
      item.classList.add('graph-plan-history-rejected');
    }
    var head = el('div', 'graph-plan-history-head');
    head.appendChild(document.createTextNode(
      'rev ' + String(change.rev) + ' ' + String(change.kind || '') +
      (change.applied ? '' : ' rejected')
    ));
    var time = formatPlanHistoryTime(change.at_ms);
    if (time) {
      head.appendChild(document.createTextNode(' \u00b7 ' + time));
    }
    item.appendChild(head);
    item.appendChild(el('div', 'graph-plan-history-delta', planHistoryDelta(change)));
    var reason = change.reason ? String(change.reason) : '';
    if (!change.applied && change.error) {
      reason = reason ? reason + ' \u2014 ' + String(change.error) : String(change.error);
    }
    if (reason) {
      item.appendChild(el('div', 'graph-plan-history-reason', reason));
    }
    list.appendChild(item);
  }
  panel.appendChild(list);
  panel.scrollTop = scrollTop;
}

function renderGraphHeaderActions(graph) {
  var replanBtn = $('graph-replan');
  var historyBtn = $('plan-history');
  if (!replanBtn || !historyBtn) { return; }
  if (!graph) {
    replanBtn.hidden = true;
    historyBtn.hidden = true;
    planHistoryUi.open = false;
    renderPlanHistoryPanel(null);
    return;
  }
  var showReplan = graphReplanAllowed(graph);
  replanBtn.hidden = !showReplan;
  var hasHistory = Array.isArray(graph.plan_history) && graph.plan_history.length > 0;
  historyBtn.hidden = !hasHistory;
  if (!hasHistory) {
    planHistoryUi.open = false;
  }
  syncSessionPlanGateButtons();
  renderPlanHistoryPanel(graph);
}

window.dmuxaiPlanGate = {
  approve: function () { submitPlanDecision('approve'); },
  reject: function () {
    if (!window.confirm('Reject this plan revision?')) { return; }
    submitPlanDecision('reject');
  },
  toggleEdit: function () {
    var graph = snapshot && snapshot.graph;
    if (!graph) { return; }
    var opening = !planGateUi.editOpen;
    planGateUi.editOpen = opening;
    if (opening) { approvalPanelFocusFeedback = true; }
    renderApprovalPanel(graph);
  },
  replan: function () { submitPlanReplan(); },
  isBusy: function () { return planGateUi.busy; }
};

var lastRenderedAgentsKey = null;

function computeAgentsKey(list) {
  var parts = [];
  for (var i = 0; i < list.length; i++) {
    var a = list[i] || {};
    var r = a.run || {};
    parts.push([
      a.id,
      a.label,
      a.color,
      a.busy ? 1 : 0,
      a.phase,
      a.model,
      a.activity,
      r.state,
      r.started_at_ms,
      r.ended_at_ms,
      r.duration_ms
    ].join('|'));
  }
  return parts.join(';;');
}

function renderAgents(agents, nowMs) {
  if (window.dmuxaiSession && typeof window.dmuxaiSession.updateAgentRunIndicators === 'function') {
    window.dmuxaiSession.updateAgentRunIndicators(agents, nowMs);
    return;
  }
  var wrap = $('agents');
  if (!wrap) { return; }
  var list = Array.isArray(agents) ? agents : [];
  var now = typeof nowMs === 'number' ? nowMs : Date.now();
  var key = computeAgentsKey(list);

  if (lastRenderedAgentsKey === key && wrap.children.length === list.length) {
    for (var j = 0; j < list.length; j++) {
      var a = list[j];
      if (a && a.run && a.run.state === 'running' && typeof a.run.started_at_ms === 'number') {
        var chipEl = wrap.children[j];
        if (chipEl) {
          var elapsedEl = chipEl.querySelector('.chip-run-elapsed');
          if (elapsedEl) {
            var ms = runElapsedMs(a.run, now);
            elapsedEl.textContent = ' ' + (fmtElapsed(ms) || '0s');
          }
        }
      }
    }
    return;
  }

  lastRenderedAgentsKey = key;
  wrap.replaceChildren();
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
    if (agent.activity) { bits.push(String(agent.activity)); }
    if (bits.length > 0) {
      var detail = bits.join(' \u00b7 ');
      chip.appendChild(el('span', 'chip-meta', detail));
    }

    if (agent.run) {
      var runState = agent.run.state;
      if (runState === 'running') {
        chip.classList.add('chip-running');
        var runEl = el('span', 'chip-run chip-run-running');
        runEl.appendChild(el('span', 'spinner'));
        var ms = runElapsedMs(agent.run, now);
        runEl.appendChild(el('span', 'chip-run-elapsed', ' ' + (fmtElapsed(ms) || '0s')));
        chip.appendChild(runEl);
      } else if (runState === 'done') {
        chip.classList.add('chip-done');
        var dur = typeof agent.run.duration_ms === 'number' ? fmtElapsed(agent.run.duration_ms) : '';
        chip.appendChild(el('span', 'chip-run chip-run-done', '✓' + (dur ? ' ' + dur : '')));
      } else if (runState === 'failed') {
        chip.classList.add('chip-failed');
        var dur = typeof agent.run.duration_ms === 'number' ? fmtElapsed(agent.run.duration_ms) : '';
        chip.appendChild(el('span', 'chip-run chip-run-failed', '✗' + (dur ? ' ' + dur : '')));
      } else if (runState === 'cancelled') {
        chip.classList.add('chip-cancelled');
        chip.appendChild(el('span', 'chip-run chip-run-cancelled', '■ cancelled'));
      }
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

// To-do sidebar filter (SPEC-todo-sidebar FR-841): state open/done/all plus
// a text search over title, body and the Jira key.
function todoFilterState() {
  var sel = $('todo-filter-state');
  var v = sel ? String(sel.value || 'open') : 'open';
  return v === 'done' || v === 'all' ? v : 'open';
}

function todoMatchesFilter(todo, state, needle) {
  if (!todo) { return false; }
  var done = String(todo.status || 'open') === 'done';
  if (state === 'open' && done) { return false; }
  if (state === 'done' && !done) { return false; }
  if (!needle) { return true; }
  var hay = [todo.title, todo.body, todo.external_key].map(function (v) {
    return String(v || '').toLowerCase();
  }).join('\n');
  return hay.indexOf(needle) !== -1;
}

// Header badge: open (not done) to-dos (FR-840).
function updateTodoCount(allTodos) {
  var badge = $('todos-count');
  if (!badge) { return; }
  var open = 0;
  for (var i = 0; i < allTodos.length; i++) {
    if (allTodos[i] && String(allTodos[i].status || 'open') !== 'done') { open++; }
  }
  badge.textContent = String(open);
  // A span's aria-label is ignored, so the count is announced via the button name.
  var toggle = $('todos-toggle');
  if (toggle) { toggle.setAttribute('aria-label', 'To-dos, ' + open + ' open'); }
}

function bindTodoFilter() {
  var rerender = function () { if (snapshot) { renderTodos(snapshot); } };
  var sel = $('todo-filter-state');
  if (sel) { sel.addEventListener('change', rerender); }
  var search = $('todo-search');
  if (search) { search.addEventListener('input', rerender); }
}

function renderTodos(snap) {
  var list = $('todo-list');
  if (!list) { return; }
  updateTodoViewOnlyHint(snap);
  var allTodos = snap && Array.isArray(snap.todos) ? snap.todos : [];
  updateTodoCount(allTodos);
  var filterState = todoFilterState();
  var searchEl = $('todo-search');
  var needle = searchEl ? String(searchEl.value || '').trim().toLowerCase() : '';
  var todos = allTodos.filter(function (t) {
    // Keep the item being edited visible so unsaved input is never lost.
    return (todoEditId && t && String(t.id) === todoEditId) ||
      todoMatchesFilter(t, filterState, needle);
  });

  var i;
  var alive = {};
  for (i = 0; i < allTodos.length; i++) {
    var todo = allTodos[i];
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
    list.appendChild(el('p', 'empty', allTodos.length === 0 ? 'no to-dos yet' : 'no to-dos match the filter'));
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

function isRunStateTerminal(state) {
  return state === 'done' || state === 'failed' || state === 'cancelled' || state === 'idle';
}

function getRunningAgents(snapshot) {
  var agents = snapshot && Array.isArray(snapshot.agents) ? snapshot.agents : [];
  return agents.filter(function (a) {
    if (!a) { return false; }
    if (a.run && a.run.state === 'running') { return true; }
    if (a.run && isRunStateTerminal(a.run.state)) { return false; }
    return Boolean(a.busy);
  });
}

function getMostRecentEndedRun(snapshot) {
  var agents = snapshot && Array.isArray(snapshot.agents) ? snapshot.agents : [];
  var ended = [];
  for (var i = 0; i < agents.length; i++) {
    var a = agents[i];
    if (a && a.run && (a.run.state === 'done' || a.run.state === 'failed' || a.run.state === 'cancelled')) {
      ended.push(a.run);
    }
  }
  if (ended.length === 0) { return null; }
  ended.sort(function (x, y) {
    var tx = typeof x.ended_at_ms === 'number' ? x.ended_at_ms : (x.started_at_ms || 0);
    var ty = typeof y.ended_at_ms === 'number' ? y.ended_at_ms : (y.started_at_ms || 0);
    return ty - tx;
  });
  return ended[0];
}

var lastRunStatusKey = null;

function updateRunStatus(snap, nowMs) {
  var pill = $('run-status');
  if (!pill) { return; }
  var runningAgents = getRunningAgents(snap);

  if (runningAgents.length > 0) {
    var agent = runningAgents[0];
    var firstLabel = agent.label || agent.id || 'agent';
    var agentText = runningAgents.length > 1
      ? firstLabel + ' +' + (runningAgents.length - 1)
      : firstLabel;
    var activityText = agent.activity ? String(agent.activity) : '';

    var elapsedStr = '0s';
    if (agent.run && typeof agent.run.started_at_ms === 'number') {
      var ms = runElapsedMs(agent.run, nowMs);
      elapsedStr = fmtElapsed(ms) || '0s';
    }

    var statusKey = 'running:' + agentText + ':' + activityText;
    if (lastRunStatusKey === statusKey && pill.classList.contains('run-status-running')) {
      var textEl = pill.querySelector('.run-status-text');
      if (textEl) {
        textEl.textContent = 'Running \u00b7 ' + agentText + ' \u00b7 ' + elapsedStr;
        return;
      }
    }

    lastRunStatusKey = statusKey;
    pill.className = 'run-status run-status-running';
    pill.replaceChildren();
    pill.appendChild(el('span', 'spinner'));
    pill.appendChild(el('span', 'run-status-text', 'Running \u00b7 ' + agentText + ' \u00b7 ' + elapsedStr));
    if (activityText) {
      pill.appendChild(el('span', 'run-status-activity', ' \u00b7 ' + activityText));
    }
  } else {
    var lastEnded = getMostRecentEndedRun(snap);
    var statusKey = 'idle';
    if (lastEnded && lastEnded.state === 'done') {
      var dur = typeof lastEnded.duration_ms === 'number'
        ? fmtElapsed(lastEnded.duration_ms)
        : (typeof lastEnded.ended_at_ms === 'number' && typeof lastEnded.started_at_ms === 'number'
          ? fmtElapsed(lastEnded.ended_at_ms - lastEnded.started_at_ms)
          : null);
      var text = dur ? '✓ Done \u00b7 ' + dur : '✓ Done';
      statusKey = 'done:' + text;
      if (lastRunStatusKey !== statusKey) {
        lastRunStatusKey = statusKey;
        pill.className = 'run-status run-status-done';
        pill.textContent = text;
      }
    } else if (lastEnded && lastEnded.state === 'failed') {
      var dur = typeof lastEnded.duration_ms === 'number'
        ? fmtElapsed(lastEnded.duration_ms)
        : (typeof lastEnded.ended_at_ms === 'number' && typeof lastEnded.started_at_ms === 'number'
          ? fmtElapsed(lastEnded.ended_at_ms - lastEnded.started_at_ms)
          : null);
      var text = dur ? '✗ Failed \u00b7 ' + dur : '✗ Failed';
      statusKey = 'failed:' + text;
      if (lastRunStatusKey !== statusKey) {
        lastRunStatusKey = statusKey;
        pill.className = 'run-status run-status-failed';
        pill.textContent = text;
      }
    } else if (lastEnded && lastEnded.state === 'cancelled') {
      statusKey = 'cancelled';
      if (lastRunStatusKey !== statusKey) {
        lastRunStatusKey = statusKey;
        pill.className = 'run-status run-status-cancelled';
        pill.textContent = '■ Cancelled';
      }
    } else {
      statusKey = 'idle';
      if (lastRunStatusKey !== statusKey) {
        lastRunStatusKey = statusKey;
        pill.className = 'run-status run-status-idle';
        pill.textContent = 'Idle';
      }
    }
  }
}

function updateTabTitle(snap) {
  var runningAgents = getRunningAgents(snap);
  if (runningAgents.length > 0) {
    document.title = '⏳ ' + baseDocumentTitle;
    lastObservedRunState = 'running';
    updateFavicon('running');
  } else {
    var lastEnded = getMostRecentEndedRun(snap);
    if (lastObservedRunState === 'running') {
      if (lastEnded && lastEnded.state === 'done') {
        document.title = '✓ ' + baseDocumentTitle;
        lastObservedRunState = 'done';
        updateFavicon('done');
      } else if (lastEnded && lastEnded.state === 'failed') {
        document.title = '✗ ' + baseDocumentTitle;
        lastObservedRunState = 'failed';
        updateFavicon('failed');
      } else {
        document.title = baseDocumentTitle;
        lastObservedRunState = 'idle';
        updateFavicon('idle');
      }
    } else if (lastObservedRunState !== 'done' && lastObservedRunState !== 'failed') {
      document.title = baseDocumentTitle;
      updateFavicon('idle');
    }
  }
}

function isSomethingRunning(snap) {
  if (!snap) { return false; }
  var agents = Array.isArray(snap.agents) ? snap.agents : [];
  for (var i = 0; i < agents.length; i++) {
    if (agents[i] && agents[i].run && agents[i].run.state === 'running') {
      return true;
    }
    if (agents[i] && agents[i].busy && (!agents[i].run || !isRunStateTerminal(agents[i].run.state))) {
      return true;
    }
  }
  var graph = snap.graph;
  if (graph) {
    if (Array.isArray(graph.current_nodes) && graph.current_nodes.length > 0) {
      return true;
    }
    if (Array.isArray(graph.nodes)) {
      for (var j = 0; j < graph.nodes.length; j++) {
        if (graphNodeStatus(graph.nodes[j]) === 'running') {
          return true;
        }
      }
    }
  }
  return false;
}

function render(nowMs) {
  var snap = snapshot;
  if (!snap || typeof snap.revision !== 'number') { return; }

  var wsEl = $('workspace-button') || $('workspace');
  if (wsEl) {
    var rawWs = String(snap.workspace || '-');
    wsEl.title = snap.workspace ? String(snap.workspace) : '';
    wsEl.textContent = typeof window.dmuxaiFormatWorkspacePath === 'function'
      ? window.dmuxaiFormatWorkspacePath(rawWs)
      : rawWs;
  }
  $('source').textContent = String(snap.source || '-');
  $('revision').textContent = String(snap.revision);

  renderAgents(snap.agents, nowMs);
  updateRunStatus(snap, nowMs);
  updateTabTitle(snap);

  var graph = snap.graph;
  if (!graph) {
    $('graph-empty').hidden = false;
    $('graph-body').hidden = true;
    // FR-1222: no text of another conversation's run stays in the hidden body.
    if ($('graph-id')) { $('graph-id').textContent = ''; }
    graphZoomGroup = null;
    lastGraphCanvasRevision = null;
    lastGraphCanvasGeometryKey = null;
    lastGraphCanvasNodesKey = null;
    renderGateHint(null);
    renderGraphHeaderActions(null);
    renderGraphJudgeLine(null);
    renderApprovalPanel(null);
    renderTaskChecklist(null);
    renderCounts(null);
    renderBar(null);
    if ($('graph-percent')) { $('graph-percent').textContent = '0%'; }
    if ($('graph-eta')) { $('graph-eta').hidden = true; $('graph-eta').textContent = ''; }
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
    renderGraphJudgeLine(graph);
    renderGraphHeaderActions(graph);
    renderApprovalPanel(graph);
    renderTaskChecklist(graph);
    renderGraphSummary(graph);
    renderNodes(graph, nowMs);
    applyGraphViewMode();
    if (graphViewMode === 'graph') {
      syncGraphCanvas(graph, nowMs, snap.revision);
      renderGraphDetail(graph, nowMs);
    } else {
      renderGraphDetail(graph, nowMs);
    }
    renderGateHint(graph);
  }
}

function tickDashboard() {
  var snap = snapshot;
  if (!snap || typeof snap.receivedAt !== 'number') { return; }
  var now = Date.now();
  if (!isSomethingRunning(snap)) {
    stopTickTimer();
    updateRunStatus(snap, now);
    updateTabTitle(snap);
    renderAgents(snap.agents, now);
    return;
  }
  updateRunStatus(snap, now);
  updateTabTitle(snap);
  renderAgents(snap.agents, now);
  var graph = snap.graph;
  if (!graph) { return; }
  renderGraphSummary(graph);
  renderNodes(graph, now);
  if (taskChecklistSelectedId) { renderTaskChecklist(graph); }
  if (graphViewMode === 'graph') {
    updateRunningGraphCards(graph, now);
    if (graphSelectedId) {
      renderGraphDetail(graph, now);
    }
  }
}

function scheduleTick() {
  if (tickTimer !== null) { return; }
  if (isSomethingRunning(snapshot)) {
    tickTimer = setInterval(tickDashboard, TICK_MS);
  }
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
  syncCenterViewsWithSnapshot(snap);
  applyPendingRunFocus(snap);
  renderTodos(snap);
  if (isSomethingRunning(snap)) {
    scheduleTick();
  } else {
    stopTickTimer();
  }
  // Optional modules (session.js) reuse this connection instead of opening
  // their own SSE stream (SPEC-dashboard-parity NFR-780).
  document.dispatchEvent(new CustomEvent('dmuxai:snapshot', { detail: snap }));
  // SPEC-docs-preview INT-754: let isolated modules (docs.js) follow snapshots.
  try { window.dispatchEvent(new CustomEvent('dmuxai:snapshot', { detail: snap })); } catch (err) { /* ignore */ }
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

  // Graph controls (toggle, pan/zoom, details) work without a token too.
  bindGraphControls();
  bindCenterViews();

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
  bindTodoFilter();
  loadImportServers();
  startSse();
}

init();
