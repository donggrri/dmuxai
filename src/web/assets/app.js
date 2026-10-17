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
      // generated-at timestamp plus the time our page held the snapshot.
      return Math.max(
        0,
        (snap.generated_at_ms - node.started_at_ms) + (nowMs - snap.receivedAt)
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
  render(lastRendered);
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
  if (!token) {
    showMsg(
      'Missing access token. Start DmuxAI and open the dashboard URL it prints ' +
      '(for example: dmuxai serve, then open http://127.0.0.1:7420/?token=...).',
      true
    );
    return;
  }

  render(Date.now());
  startSse();
}

init();
