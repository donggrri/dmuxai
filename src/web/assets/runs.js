'use strict';

/* FR-2811..FR-2814: headless runs dashboard view (read-only). */

var RUNS_POLL_MS = 2000;
var RUNS_TICK_MS = 1000;
var RUNS_STREAM_MAX_LINES = 2000;
var RUNS_STREAM_BACKOFF = [1000, 2000, 5000, 10000];
var RUNS_TOKEN_KEY = 'dmuxai.token';
var RUNS_EMPTY_HINT =
  'No headless runs yet. Keep `dmuxai serve` running in this folder, then start `dmuxai run` here.';

function runsToken() {
  try {
    return window.sessionStorage.getItem(RUNS_TOKEN_KEY);
  } catch (err) {
    return null;
  }
}

function runsApiUrl(path) {
  if (typeof window.dmuxaiApiUrl === 'function') {
    return window.dmuxaiApiUrl(path);
  }
  return path;
}

function runsEl(tag, className, text) {
  var node = document.createElement(tag);
  if (className) { node.className = className; }
  if (text !== undefined && text !== null) {
    node.textContent = String(text);
  }
  return node;
}

function runsShortId(id) {
  if (!id || id.length < 8) { return id || ''; }
  return id.slice(0, 8);
}

function runsOriginLabel(origin) {
  if (origin === 'cli_headless') { return 'cli'; }
  if (origin === 'mcp') { return 'mcp'; }
  return origin || '';
}

function runsRunnerModel(run) {
  var runner = run.resolved_runner || run.requested_runner || '';
  var model = run.resolved_model || run.requested_model || '';
  if (runner && model) { return runner + ' / ' + model; }
  return runner || model || '\u2013';
}

function runsFormatElapsed(ms) {
  if (typeof ms !== 'number' || ms < 0 || !isFinite(ms)) { return '0:00'; }
  var totalSec = Math.floor(ms / 1000);
  var m = Math.floor(totalSec / 60);
  var s = totalSec % 60;
  return m + ':' + (s < 10 ? '0' : '') + s;
}

function runsFormatClock(tsMs) {
  if (typeof tsMs !== 'number' || tsMs <= 0) { return ''; }
  try {
    var d = new Date(tsMs);
    var h = d.getHours();
    var min = d.getMinutes();
    var sec = d.getSeconds();
    return (h < 10 ? '0' : '') + h + ':' +
      (min < 10 ? '0' : '') + min + ':' +
      (sec < 10 ? '0' : '') + sec;
  } catch (err) {
    return '';
  }
}

function runsFormatLocalStart(tsMs) {
  if (typeof tsMs !== 'number' || tsMs <= 0) { return '\u2013'; }
  try {
    return new Date(tsMs).toLocaleString();
  } catch (err) {
    return '\u2013';
  }
}

function runsIsActive(status) {
  return status === 'active' || status === 'running';
}

function runsFetchJson(path) {
  var tok = runsToken();
  return fetch(runsApiUrl(path), {
    headers: tok ? { Authorization: 'Bearer ' + tok } : {},
    cache: 'no-store'
  }).then(function (res) {
    if (!res.ok) {
      throw new Error('HTTP ' + res.status);
    }
    return res.json();
  });
}

function runsCopyText(text) {
  if (typeof navigator !== 'undefined' && navigator.clipboard && window.isSecureContext) {
    return navigator.clipboard.writeText(text);
  }
  return new Promise(function (resolve, reject) {
    var ta = document.createElement('textarea');
    ta.value = text;
    ta.setAttribute('readonly', '');
    ta.style.position = 'fixed';
    ta.style.left = '-9999px';
    document.body.appendChild(ta);
    ta.select();
    try {
      var ok = document.execCommand('copy');
      document.body.removeChild(ta);
      if (ok) { resolve(); } else { reject(new Error('copy failed')); }
    } catch (err) {
      document.body.removeChild(ta);
      reject(err);
    }
  });
}

var runsUiOpen = false;
var runsPollTimer = null;
var runsTickTimer = null;
var runsEventsSse = null;
var runsStreamSse = null;
var runsListData = [];
var runsSelectedId = null;
var runsDetailRun = null;
var runsStreamLines = 0;
var runsStreamStickBottom = true;
var runsStreamBackoffIx = 0;
var runsStreamReconnectTimer = null;

function runsPanelOpen() {
  var panel = document.getElementById('runs');
  return !!(panel && !panel.hidden && panel.classList.contains('open'));
}

function runsStopPoll() {
  if (runsPollTimer !== null) {
    clearInterval(runsPollTimer);
    runsPollTimer = null;
  }
}

function runsStopTick() {
  if (runsTickTimer !== null) {
    clearInterval(runsTickTimer);
    runsTickTimer = null;
  }
}

function runsStopEventsSse() {
  if (runsEventsSse) {
    runsEventsSse.close();
    runsEventsSse = null;
  }
}

function runsStopStream() {
  if (runsStreamReconnectTimer !== null) {
    clearTimeout(runsStreamReconnectTimer);
    runsStreamReconnectTimer = null;
  }
  if (runsStreamSse) {
    runsStreamSse.close();
    runsStreamSse = null;
  }
  runsStreamBackoffIx = 0;
}

function runsStartPoll() {
  runsStopPoll();
  runsRefreshList();
  runsPollTimer = window.setInterval(runsRefreshList, RUNS_POLL_MS);
}

function runsStartTick() {
  runsStopTick();
  runsTickTimer = window.setInterval(runsTickElapsed, RUNS_TICK_MS);
}

function runsStartEventsSse() {
  runsStopEventsSse();
  if (typeof EventSource === 'undefined') { return; }
  var tok = runsToken();
  if (!tok) { return; }
  try {
    runsEventsSse = new EventSource(
      runsApiUrl('/api/events?token=' + encodeURIComponent(tok))
    );
  } catch (err) {
    return;
  }
  runsEventsSse.addEventListener('state-change', function () {
    runsRefreshList();
    if (runsSelectedId) {
      runsLoadDetail(runsSelectedId, false);
    }
  });
}

function runsOnViewOpen() {
  if (runsUiOpen) { return; }
  runsUiOpen = true;
  runsStartPoll();
  runsStartTick();
  runsStartEventsSse();
}

function runsOnViewClose() {
  if (!runsUiOpen) { return; }
  runsUiOpen = false;
  runsStopPoll();
  runsStopTick();
  runsStopEventsSse();
  runsStopStream();
}

function runsRefreshList() {
  runsFetchJson('/api/runs').then(function (data) {
    runsListData = data && data.runs ? data.runs : [];
    runsRenderList();
  }).catch(function () { /* keep last list */ });
}

function runsRenderList() {
  var table = document.getElementById('runs-table');
  var tbody = document.getElementById('runs-table-body');
  var empty = document.getElementById('runs-empty');
  if (!table || !tbody || !empty) { return; }

  tbody.textContent = '';
  if (!runsListData.length) {
    table.hidden = true;
    empty.hidden = false;
    empty.textContent = RUNS_EMPTY_HINT;
    runsClearDetail();
    return;
  }
  empty.hidden = true;
  table.hidden = false;

  runsListData.forEach(function (run) {
    var tr = document.createElement('tr');
    tr.className = 'runs-row';
    tr.setAttribute('data-run-id', run.run_id);
    tr.setAttribute('role', 'option');
    var selected = run.run_id === runsSelectedId;
    tr.setAttribute('aria-selected', selected ? 'true' : 'false');
    if (selected) { tr.classList.add('runs-row-selected'); }

    var tdStatus = document.createElement('td');
    var badge = runsEl('span', 'badge run-status run-status-' + run.status, run.status);
    tdStatus.appendChild(badge);
    tr.appendChild(tdStatus);

    var tdOrigin = runsEl('td', 'runs-col-origin', runsOriginLabel(run.origin));
    tr.appendChild(tdOrigin);

    var tdRm = runsEl('td', 'runs-col-runner', runsRunnerModel(run));
    tr.appendChild(tdRm);

    var tdElapsed = runsEl('td', 'runs-col-elapsed runs-elapsed');
    tdElapsed.setAttribute('data-run-id', run.run_id);
    tdElapsed.setAttribute('data-created', String(run.created_at_ms || 0));
    tdElapsed.setAttribute('data-active', runsIsActive(run.status) ? '1' : '0');
    tdElapsed.setAttribute('data-elapsed', String(run.elapsed_ms || 0));
    tdElapsed.textContent = runsFormatElapsed(run.elapsed_ms || 0);
    tr.appendChild(tdElapsed);

    var tdFlags = document.createElement('td');
    tdFlags.className = 'runs-col-flags';
    if (run.stale) {
      tdFlags.appendChild(runsEl('span', 'badge runs-flag-stale', 'stale'));
    }
    if (run.stalled) {
      tdFlags.appendChild(runsEl('span', 'badge runs-flag-stalled', 'stalled'));
    }
    tr.appendChild(tdFlags);

    tr.addEventListener('click', function () {
      runsSelectRun(run.run_id);
    });
    tbody.appendChild(tr);
  });

  if (!runsSelectedId && runsListData.length) {
    runsSelectRun(runsListData[0].run_id);
  }
}

function runsTickElapsed() {
  var cells = document.querySelectorAll('.runs-elapsed[data-active="1"]');
  var now = Date.now();
  for (var i = 0; i < cells.length; i++) {
    var cell = cells[i];
    var created = parseInt(cell.getAttribute('data-created'), 10);
    if (!isFinite(created) || created <= 0) { continue; }
    var ms = now - created;
    cell.textContent = runsFormatElapsed(ms);
  }
  if (runsDetailRun && runsIsActive(runsDetailRun.status)) {
    runsUpdateDetailMeta();
  }
}

function runsSelectRun(id) {
  runsSelectedId = id;
  runsRenderList();
  runsLoadDetail(id, true);
}

function runsClearDetail() {
  runsSelectedId = null;
  runsDetailRun = null;
  runsStopStream();
  var detail = document.getElementById('runs-detail');
  if (detail) { detail.hidden = true; }
}

function runsLoadDetail(id, reopenStream) {
  runsFetchJson('/api/runs/' + encodeURIComponent(id)).then(function (data) {
    runsDetailRun = data.run || null;
    var detail = document.getElementById('runs-detail');
    if (detail) { detail.hidden = false; }
    runsRenderDetailHeader(data);
    runsRenderApproval(data.approval);
    runsRenderGraphNodes(data.graph);
    if (reopenStream) {
      runsRenderInitialProgress(data.progress);
      runsConnectStream(id);
    }
  }).catch(function () { /* keep prior detail */ });
}

function runsRenderDetailHeader(data) {
  var header = document.getElementById('runs-detail-header');
  if (!header) { return; }
  header.textContent = '';
  var run = data.run || {};
  runsDetailRun = run;

  var title = runsEl('h3', 'runs-detail-title',
    runsShortId(run.run_id) + ' \u00b7 ' + (run.status || ''));
  header.appendChild(title);

  var meta = runsEl('div', 'runs-detail-meta');
  var lines = [
    'Runner / model: ' + runsRunnerModel(run),
    'Origin: ' + runsOriginLabel(run.origin),
    'Started: ' + runsFormatLocalStart(run.created_at_ms),
    'Elapsed: ' + runsFormatElapsed(run.elapsed_ms || 0),
  ];
  if (run.dmuxai_exit_code !== null && run.dmuxai_exit_code !== undefined) {
    lines.push('Exit code: ' + String(run.dmuxai_exit_code));
  }
  if (run.error_kind) {
    lines.push('Error kind: ' + String(run.error_kind));
  }
  if (run.error_summary) {
    lines.push('Error: ' + String(run.error_summary));
  }
  lines.forEach(function (line) {
    meta.appendChild(runsEl('div', 'runs-detail-line', line));
  });
  header.appendChild(meta);
  header.setAttribute('data-run-id', run.run_id || '');
}

function runsUpdateDetailMeta() {
  if (!runsDetailRun) { return; }
  var header = document.getElementById('runs-detail-header');
  if (!header) { return; }
  var meta = header.querySelector('.runs-detail-meta');
  if (!meta) { return; }
  var lines = meta.querySelectorAll('.runs-detail-line');
  if (lines.length < 4) { return; }
  var elapsedMs = runsDetailRun.elapsed_ms || 0;
  if (runsIsActive(runsDetailRun.status)) {
    elapsedMs = Date.now() - (runsDetailRun.created_at_ms || 0);
  }
  if (lines[3]) {
    lines[3].textContent = 'Elapsed: ' + runsFormatElapsed(elapsedMs);
  }
}

function runsRenderApproval(approval) {
  var banner = document.getElementById('runs-approval-banner');
  if (!banner) { return; }
  banner.textContent = '';
  if (!approval || !approval.hint) {
    banner.hidden = true;
    return;
  }
  banner.hidden = false;
  banner.appendChild(document.createTextNode(
    'Waiting for approval \u2014 this run has exited; re-run to approve: '));
  var code = runsEl('code', 'runs-approval-hint', approval.hint);
  banner.appendChild(code);
  var copyBtn = runsEl('button', 'btn btn-small runs-approval-copy', 'Copy');
  copyBtn.type = 'button';
  copyBtn.addEventListener('click', function () {
    runsCopyText(approval.hint).catch(function () { /* ignore */ });
  });
  banner.appendChild(copyBtn);
}

function runsRenderGraphNodes(graph) {
  var section = document.getElementById('runs-graph-section');
  var list = document.getElementById('runs-graph-nodes');
  if (!section || !list) { return; }
  list.textContent = '';
  if (!graph || !graph.nodes || !graph.nodes.length) {
    section.hidden = true;
    return;
  }
  section.hidden = false;
  graph.nodes.forEach(function (node) {
    var li = document.createElement('li');
    li.className = 'runs-graph-node';
    var title = node.job || node.id || '';
    li.appendChild(runsEl('span', 'runs-graph-node-id', node.id + ' / ' + title));
    li.appendChild(runsEl('span', 'runs-graph-node-runner', node.runner || ''));
    li.appendChild(runsEl('span', 'badge run-status run-status-' + node.status, node.status));
    list.appendChild(li);
  });
}

function runsStreamOl() {
  return document.getElementById('runs-stream');
}

function runsTrimStream() {
  var ol = runsStreamOl();
  if (!ol) { return; }
  while (ol.children.length > RUNS_STREAM_MAX_LINES) {
    ol.removeChild(ol.firstChild);
    runsStreamLines -= 1;
  }
}

function runsAppendStreamLine(className, parts) {
  var ol = runsStreamOl();
  if (!ol) { return; }
  var li = document.createElement('li');
  li.className = className;
  parts.forEach(function (part, ix) {
    if (ix > 0) { li.appendChild(document.createTextNode(' \u00b7 ')); }
    li.appendChild(runsEl('span', 'runs-stream-part', part));
  });
  ol.appendChild(li);
  runsStreamLines += 1;
  runsTrimStream();
  if (runsStreamStickBottom) {
    var wrap = document.getElementById('runs-stream-wrap');
    if (wrap) { wrap.scrollTop = wrap.scrollHeight; }
  }
}

function runsParseProgressLine(raw) {
  try {
    return JSON.parse(raw);
  } catch (err) {
    return { summary: raw, kind: 'line' };
  }
}

function runsRenderProgressItem(item) {
  if (!item || typeof item !== 'object') { return; }
  var ts = item.ts_ms ? runsFormatClock(item.ts_ms) : '';
  var kind = item.kind ? String(item.kind) : 'progress';
  var summary = item.summary ? String(item.summary) : JSON.stringify(item);
  runsAppendStreamLine('runs-stream-progress', [ts, kind, summary]);
}

function runsRenderInitialProgress(progress) {
  var ol = runsStreamOl();
  if (ol) { ol.textContent = ''; }
  runsStreamLines = 0;
  runsStreamStickBottom = true;
  if (!progress || !progress.length) { return; }
  progress.forEach(runsRenderProgressItem);
}

function runsConnectStream(id) {
  runsStopStream();
  if (typeof EventSource === 'undefined') { return; }
  var tok = runsToken();
  if (!tok) { return; }
  var path = '/api/runs/' + encodeURIComponent(id) + '/stream?token=' + encodeURIComponent(tok);
  try {
    runsStreamSse = new EventSource(runsApiUrl(path));
  } catch (err) {
    return;
  }

  runsStreamSse.addEventListener('run', function (e) {
    try {
      var run = JSON.parse(e.data);
      runsDetailRun = run;
      runsRenderDetailHeader({ run: run });
      runsListData = runsListData.map(function (item) {
        return item.run_id === run.run_id ? Object.assign({}, item, run) : item;
      });
      runsRenderList();
      if (runsSelectedId === run.run_id) {
        runsLoadDetail(run.run_id, false);
      }
    } catch (err) { /* ignore */ }
  });

  runsStreamSse.addEventListener('progress', function (e) {
    runsRenderProgressItem(runsParseProgressLine(e.data));
  });

  runsStreamSse.addEventListener('state-change', function (e) {
    var rec;
    try { rec = JSON.parse(e.data); } catch (err) { return; }
    var kind = rec.kind ? String(rec.kind) : '';
    var toState = rec.to_state ? String(rec.to_state) : '';
    var msg = rec.message ? String(rec.message) : '';
    runsAppendStreamLine('runs-stream-state', [kind, toState, msg]);
  });

  runsStreamSse.addEventListener('end', function () {
    runsStopStream();
  });

  runsStreamSse.addEventListener('error', function () {
    if (!runsDetailRun || !runsIsActive(runsDetailRun.status)) {
      runsStopStream();
      return;
    }
    if (runsStreamSse) {
      runsStreamSse.close();
      runsStreamSse = null;
    }
    var delay = RUNS_STREAM_BACKOFF[Math.min(runsStreamBackoffIx, RUNS_STREAM_BACKOFF.length - 1)];
    runsStreamBackoffIx += 1;
    runsStreamReconnectTimer = window.setTimeout(function () {
      runsStreamReconnectTimer = null;
      if (runsSelectedId === id && runsUiOpen) {
        runsConnectStream(id);
      }
    }, delay);
  });
}

function runsBindStreamScroll() {
  var wrap = document.getElementById('runs-stream-wrap');
  if (!wrap) { return; }
  wrap.addEventListener('scroll', function () {
    var gap = wrap.scrollHeight - wrap.scrollTop - wrap.clientHeight;
    runsStreamStickBottom = gap < 40;
  });
}

function runsSyncViewState() {
  if (runsPanelOpen()) {
    runsOnViewOpen();
  } else {
    runsOnViewClose();
  }
}

function runsInit() {
  runsBindStreamScroll();
  document.addEventListener('dmuxai:center-view', function () {
    runsSyncViewState();
  });
  window.setTimeout(runsSyncViewState, 0);
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', runsInit);
} else {
  runsInit();
}