'use strict';

/* FR-2811..FR-2814: headless runs dashboard view (read-only). */

var RUNS_POLL_MS = 2000;
var RUNS_INDICATOR_POLL_MS = 2000;
var RUNS_TICK_MS = 1000;
var RUNS_DETAIL_REFETCH_MS = 3000;
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

var RUNS_STATUS_LABELS = {
  active: 'Active',
  running: 'Running',
  awaiting_approval: 'Awaiting approval',
  completed: 'Completed',
  failed: 'Failed',
  cancelled: 'Cancelled',
  approved: 'Approved',
  pending: 'Pending',
  process_exit: 'Process exit',
  stale: 'Stale',
  stalled: 'Stalled'
};

function runsStatusLabel(token) {
  if (token === null || token === undefined) { return ''; }
  var raw = String(token);
  if (!raw) { return ''; }
  if (Object.prototype.hasOwnProperty.call(RUNS_STATUS_LABELS, raw)) {
    return RUNS_STATUS_LABELS[raw];
  }
  var spaced = raw.replace(/_/g, ' ');
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

window.dmuxaiStatusLabel = runsStatusLabel;

function runsIsActive(status) {
  return status === 'active' || status === 'running';
}

function runsIsLive(run) {
  return !!(run && runsIsActive(run.status) && !run.stale);
}

function runsIsTerminal(status) {
  return status && !runsIsActive(status);
}

function runsRunGraphId(run) {
  if (!run) { return ''; }
  if (run.graph_id) { return String(run.graph_id); }
  if (run.links && run.links.graph_id) { return String(run.links.graph_id); }
  return '';
}

function runsStopDetailRefetchPoll() {
  if (runsDetailRefetchTimer !== null) {
    clearInterval(runsDetailRefetchTimer);
    runsDetailRefetchTimer = null;
  }
}

function runsStreamRunNeedsRefetch(streamRun, prev) {
  if (!streamRun || streamRun.run_id !== runsSelectedId) { return false; }
  if (runsIsActive(streamRun.status)) {
    if (runsRunnerModel(streamRun) !== runsRunnerModel(prev || {})) { return true; }
    var gid = runsRunGraphId(streamRun);
    var prevGid = runsRunGraphId(prev);
    return !!(gid && gid !== prevGid);
  }
  if (prev && runsIsActive(prev.status) && runsIsTerminal(streamRun.status)) {
    return true;
  }
  return streamRun.status === 'awaiting_approval';
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

window.dmuxaiFetchRunsList = function () {
  return runsFetchJson('/api/runs').then(function (data) {
    return data && data.runs ? data.runs : [];
  });
};

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
var runsStreamSse = null;
var runsListData = [];
var runsSelectedId = null;
var runsDetailRun = null;
var runsDetailGraph = null;
var runsDetailGen = 0;
var runsStreamLines = 0;
var runsStreamTextSummaries = [];
var runsStreamStickBottom = true;
var runsStreamBackoffIx = 0;
var runsStreamReconnectTimer = null;
var runsDetailRefetchTimer = null;
var runsDetailLastRefetchAt = 0;
var runsIndicatorTimer = null;
var runsIndicatorLastFetchAt = 0;
var runsIndicatorInFlight = false;

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

function runsCloseStreamSse() {
  if (runsStreamReconnectTimer !== null) {
    clearTimeout(runsStreamReconnectTimer);
    runsStreamReconnectTimer = null;
  }
  if (runsStreamSse) {
    runsStreamSse.close();
    runsStreamSse = null;
  }
}

function runsResetStreamBackoff() {
  runsStreamBackoffIx = 0;
}

function runsStopStream() {
  runsCloseStreamSse();
  runsResetStreamBackoff();
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

function runsOnDashboardStateChange() {
  runsRefreshList();
  if (runsSelectedId) {
    runsLoadDetail(runsSelectedId, false);
  }
}

function runsOnViewOpen() {
  if (runsUiOpen) { return; }
  runsUiOpen = true;
  runsStartPoll();
  runsStartTick();
}

function runsOnViewClose() {
  if (!runsUiOpen) { return; }
  runsUiOpen = false;
  runsStopPoll();
  runsStopTick();
  runsStopDetailRefetchPoll();
  runsStopStream();
}

window.dmuxaiRunsOnViewClose = runsOnViewClose;

function runsRefreshList() {
  runsFetchJson('/api/runs').then(function (data) {
    runsListData = data && data.runs ? data.runs : [];
    runsIndicatorLastFetchAt = Date.now();
    runsRenderList();
  }).catch(function () { /* keep last list */ });
}

// FR-2826: the Runs button shows how many headless runs are active.
// FR-2846: stale runs are excluded from active counts.
function runsActiveCount() {
  var count = 0;
  for (var i = 0; i < runsListData.length; i++) {
    if (runsIsLive(runsListData[i])) { count += 1; }
  }
  return count;
}

// FR-2840: export live count and newest run status for the top status pill.
window.dmuxaiHeadlessRunsSummary = function () {
  var live = 0;
  var newest = null;
  var newestTs = -1;
  for (var i = 0; i < runsListData.length; i++) {
    var r = runsListData[i];
    if (!r) { continue; }
    if (runsIsLive(r)) { live += 1; }
    var ts = typeof r.created_at_ms === 'number' && isFinite(r.created_at_ms) ? r.created_at_ms : 0;
    if (!newest || ts > newestTs) {
      newest = r;
      newestTs = ts;
    }
  }
  return {
    live: live,
    newestStatus: newest && newest.status ? String(newest.status) : null
  };
};

function runsNotifyRunStatus() {
  if (typeof window.dmuxaiRefreshRunStatus === 'function') {
    window.dmuxaiRefreshRunStatus();
  }
}

function runsUpdateIndicator() {
  var ind = document.getElementById('runs-indicator');
  var toggle = document.getElementById('runs-toggle');
  if (ind && toggle) {
    var count = runsActiveCount();
    if (count > 0) {
      ind.hidden = false;
      ind.textContent = String(count);
      toggle.setAttribute('aria-label', 'Runs, ' + count + ' active');
    } else {
      ind.hidden = true;
      ind.textContent = '';
      toggle.setAttribute('aria-label', 'Runs');
    }
  }
  runsNotifyRunStatus();
}

// While Runs is closed, reuse GET /api/runs at most once every 2 s so the
// indicator still reflects active runs without a new endpoint.
function runsRefreshIndicator() {
  if (runsUiOpen || runsIndicatorInFlight) { return; }
  var now = Date.now();
  if (now - runsIndicatorLastFetchAt < RUNS_INDICATOR_POLL_MS) { return; }
  runsIndicatorLastFetchAt = now;
  runsIndicatorInFlight = true;
  runsFetchJson('/api/runs').then(function (data) {
    runsListData = data && data.runs ? data.runs : [];
    runsUpdateIndicator();
  }).catch(function () { /* keep last count */ }).then(function () {
    runsIndicatorInFlight = false;
  });
}

function runsRenderList() {
  runsUpdateIndicator();
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
    var rawStatus = run.status ? String(run.status) : '';
    var badge = runsEl('span', 'badge run-status run-status-' + rawStatus, runsStatusLabel(rawStatus));
    badge.setAttribute('data-status', rawStatus);
    tdStatus.appendChild(badge);
    tr.appendChild(tdStatus);

    var tdOrigin = runsEl('td', 'runs-col-origin', runsOriginLabel(run.origin));
    tr.appendChild(tdOrigin);

    var tdRm = runsEl('td', 'runs-col-runner', runsRunnerModel(run));
    tr.appendChild(tdRm);

    var tdElapsed = runsEl('td', 'runs-col-elapsed runs-elapsed');
    tdElapsed.setAttribute('data-run-id', run.run_id);
    tdElapsed.setAttribute('data-created', String(run.created_at_ms || 0));
    tdElapsed.setAttribute('data-active', runsIsLive(run) ? '1' : '0');
    tdElapsed.setAttribute('data-elapsed', String(run.elapsed_ms || 0));
    tdElapsed.textContent = runsFormatElapsed(run.elapsed_ms || 0);
    tr.appendChild(tdElapsed);

    var tdFlags = document.createElement('td');
    tdFlags.className = 'runs-col-flags';
    if (run.stale) {
      var staleBadge = runsEl('span', 'badge runs-flag-stale', runsStatusLabel('stale'));
      staleBadge.setAttribute('data-status', 'stale');
      tdFlags.appendChild(staleBadge);
    }
    if (run.stalled) {
      var stalledBadge = runsEl('span', 'badge runs-flag-stalled', runsStatusLabel('stalled'));
      stalledBadge.setAttribute('data-status', 'stalled');
      tdFlags.appendChild(stalledBadge);
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
  if (runsDetailRun && runsIsLive(runsDetailRun)) {
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
  runsDetailGraph = null;
  runsStopDetailRefetchPoll();
  runsStopStream();
  runsRenderGraphNodes(null);
  var detail = document.getElementById('runs-detail');
  if (detail) { detail.hidden = true; }
}

function runsApplyDetailPayload(data, id, gen) {
  if (gen !== runsDetailGen || id !== runsSelectedId) { return false; }
  runsDetailRun = data.run || null;
  var detail = document.getElementById('runs-detail');
  if (detail) { detail.hidden = false; }
  runsRenderDetailHeader(data);
  runsRenderApproval(data.approval, data.run);
  runsRenderGraphNodes(data.graph);
  return true;
}

function runsRefetchDetail(id) {
  var gen = runsDetailGen;
  runsFetchJson('/api/runs/' + encodeURIComponent(id)).then(function (data) {
    if (!runsApplyDetailPayload(data, id, gen)) { return; }
    if (runsRunGraphId(runsDetailRun)) {
      runsStopDetailRefetchPoll();
    }
  }).catch(function () { /* keep prior detail */ });
}

function runsMaybeStartDetailRefetchPoll(id) {
  runsStopDetailRefetchPoll();
  if (!runsDetailRun || !runsIsActive(runsDetailRun.status)) { return; }
  if (runsRunGraphId(runsDetailRun)) { return; }
  runsDetailRefetchTimer = window.setInterval(function () {
    if (id !== runsSelectedId || !runsUiOpen) {
      runsStopDetailRefetchPoll();
      return;
    }
    if (!runsDetailRun || !runsIsActive(runsDetailRun.status)) {
      runsStopDetailRefetchPoll();
      return;
    }
    if (runsRunGraphId(runsDetailRun)) {
      runsStopDetailRefetchPoll();
      return;
    }
    var now = Date.now();
    if (now - runsDetailLastRefetchAt < RUNS_DETAIL_REFETCH_MS) { return; }
    runsDetailLastRefetchAt = now;
    runsRefetchDetail(id);
  }, RUNS_DETAIL_REFETCH_MS);
}

function runsLoadDetail(id, reopenStream) {
  runsDetailGen += 1;
  var gen = runsDetailGen;
  runsDetailLastRefetchAt = 0;
  runsFetchJson('/api/runs/' + encodeURIComponent(id)).then(function (data) {
    if (!runsApplyDetailPayload(data, id, gen)) { return; }
    if (reopenStream && id === runsSelectedId) {
      runsConnectStream(id);
    }
    runsMaybeStartDetailRefetchPoll(id);
  }).catch(function () { /* keep prior detail */ });
}

function runsRenderDetailHeader(data) {
  var header = document.getElementById('runs-detail-header');
  if (!header) { return; }
  header.textContent = '';
  var run = data.run || {};
  runsDetailRun = run;

  var title = runsEl('h3', 'runs-detail-title',
    runsShortId(run.run_id) + ' \u00b7 ' + runsStatusLabel(run.status || ''));
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
    lines.push('Error kind: ' + runsStatusLabel(run.error_kind));
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
  if (runsIsLive(runsDetailRun)) {
    elapsedMs = Date.now() - (runsDetailRun.created_at_ms || 0);
  }
  if (lines[3]) {
    lines[3].textContent = 'Elapsed: ' + runsFormatElapsed(elapsedMs);
  }
}

function runsRenderApproval(approval, run) {
  var banner = document.getElementById('runs-approval-banner');
  if (!banner) { return; }
  banner.textContent = '';
  banner.className = 'runs-approval-banner';
  var detailRun = run || runsDetailRun || {};
  if (detailRun.status === 'approved') {
    var contId = detailRun.approved_by_run_id || '';
    if (!contId) {
      banner.hidden = true;
      return;
    }
    banner.hidden = false;
    banner.appendChild(document.createTextNode(
      'Approved \u2014 continued in run '));
    var link = runsEl('button', 'runs-approved-link', runsShortId(contId));
    link.type = 'button';
    link.addEventListener('click', function () {
      runsSelectRun(contId);
    });
    banner.appendChild(link);
    return;
  }
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
  var canvas = document.getElementById('runs-graph-canvas');
  var fullBtn = document.getElementById('runs-full-graph');
  var list = document.getElementById('runs-graph-nodes');
  if (!section || !list) { return; }
  list.textContent = '';
  if (!graph || !graph.nodes || !graph.nodes.length) {
    runsDetailGraph = null;
    section.hidden = true;
    if (fullBtn) { fullBtn.hidden = true; }
    if (canvas) {
      canvas.replaceChildren();
      canvas.style.height = '';
    }
    return;
  }
  runsDetailGraph = graph;
  section.hidden = false;
  if (fullBtn) { fullBtn.hidden = false; }
  if (canvas && typeof window.dmuxaiRenderGraphDiagram === 'function') {
    window.dmuxaiRenderGraphDiagram(canvas, graph, Date.now());
  }
  graph.nodes.forEach(function (node) {
    var li = document.createElement('li');
    li.className = 'runs-graph-node';
    var title = node.job || node.id || '';
    li.appendChild(runsEl('span', 'runs-graph-node-id', node.id + ' / ' + title));
    li.appendChild(runsEl('span', 'runs-graph-node-runner', node.runner || ''));
    var nodeStatus = node.status ? String(node.status) : '';
    var nodeBadge = runsEl('span', 'badge run-status run-status-' + nodeStatus, runsStatusLabel(nodeStatus));
    nodeBadge.setAttribute('data-status', nodeStatus);
    li.appendChild(nodeBadge);
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
  }
  runsStreamLines = ol.children.length;
}

function runsStreamLiTs(li) {
  var raw = li && li.getAttribute ? li.getAttribute('data-ts') : null;
  if (!raw) { return Infinity; }
  var n = Number(raw);
  return isFinite(n) && n > 0 ? n : Infinity;
}

function runsInsertStreamLi(ol, li, tsMs) {
  var myTs = (typeof tsMs === 'number' && isFinite(tsMs) && tsMs > 0) ? tsMs : Infinity;
  if (myTs < Infinity) {
    li.setAttribute('data-ts', String(myTs));
  }
  if (myTs === Infinity || !ol.lastElementChild || runsStreamLiTs(ol.lastElementChild) <= myTs) {
    ol.appendChild(li);
    return;
  }
  var cur = ol.lastElementChild;
  while (cur && runsStreamLiTs(cur) > myTs) {
    cur = cur.previousElementSibling;
  }
  if (cur) {
    ol.insertBefore(li, cur.nextSibling);
  } else {
    ol.insertBefore(li, ol.firstElementChild);
  }
}

function runsHasTextSummary(needle) {
  if (!needle) { return false; }
  for (var i = 0; i < runsStreamTextSummaries.length; i++) {
    if (runsStreamTextSummaries[i].indexOf(needle) !== -1) { return true; }
  }
  return false;
}

function runsRemoveMatchingOutputs(ol, summary) {
  if (!ol || !summary) { return; }
  var outputs = ol.querySelectorAll('li[data-stream-output="1"]');
  for (var i = 0; i < outputs.length; i++) {
    var outText = outputs[i].getAttribute('data-output-text') || '';
    if (outText && summary.indexOf(outText) !== -1) {
      ol.removeChild(outputs[i]);
    }
  }
}

function runsAppendStreamLine(className, tsMs, kindLabel, text, opts) {
  var ol = runsStreamOl();
  if (!ol) { return; }
  var timeStr = (typeof tsMs === 'number' && isFinite(tsMs) && tsMs > 0)
    ? runsFormatClock(tsMs) : '';
  var kindStr = kindLabel ? String(kindLabel) : '';
  var textStr = text !== undefined && text !== null ? String(text) : '';
  if (!kindStr && !textStr) { return; }

  var li = document.createElement('li');
  li.className = className;
  if (opts && opts.outputText) {
    li.setAttribute('data-stream-output', '1');
    li.setAttribute('data-output-text', opts.outputText);
  }
  li.appendChild(runsEl('span', 'runs-stream-time', timeStr));
  li.appendChild(runsEl('span', 'runs-stream-kind', kindStr));
  li.appendChild(runsEl('span', 'runs-stream-text', textStr));

  runsInsertStreamLi(ol, li, tsMs);
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
  var tsMs = typeof item.ts_ms === 'number' && isFinite(item.ts_ms) ? item.ts_ms : 0;
  var kind = item.kind ? String(item.kind) : 'progress';
  var summary = item.summary !== undefined && item.summary !== null
    ? String(item.summary) : JSON.stringify(item);
  if (kind === 'text' && summary) {
    runsStreamTextSummaries.push(summary);
    if (runsStreamTextSummaries.length > RUNS_STREAM_MAX_LINES) {
      runsStreamTextSummaries.shift();
    }
    runsRemoveMatchingOutputs(runsStreamOl(), summary);
  }
  runsAppendStreamLine('runs-stream-progress', tsMs, kind, summary);
}

function runsConnectStream(id) {
  if (id !== runsSelectedId) { return; }
  runsCloseStreamSse();
  var ol = runsStreamOl();
  if (ol) { ol.textContent = ''; }
  runsStreamLines = 0;
  runsStreamTextSummaries = [];
  runsStreamStickBottom = true;
  if (typeof EventSource === 'undefined') { return; }
  var tok = runsToken();
  if (!tok) { return; }
  var path = '/api/runs/' + encodeURIComponent(id) + '/stream?token=' + encodeURIComponent(tok);
  try {
    runsStreamSse = new EventSource(runsApiUrl(path));
  } catch (err) {
    return;
  }

  runsStreamSse.addEventListener('open', function (e) {
    if (e.target !== runsStreamSse) { return; }
    runsResetStreamBackoff();
  });

  runsStreamSse.addEventListener('run', function (e) {
    try {
      var run = JSON.parse(e.data);
      var prev = runsDetailRun;
      runsDetailRun = run;
      runsRenderDetailHeader({ run: run });
      runsListData = runsListData.map(function (item) {
        return item.run_id === run.run_id ? Object.assign({}, item, run) : item;
      });
      runsRenderList();
      if (runsStreamRunNeedsRefetch(run, prev)) {
        runsDetailLastRefetchAt = Date.now();
        runsRefetchDetail(run.run_id);
      } else if (runsIsActive(run.status) && !runsRunGraphId(run)) {
        runsMaybeStartDetailRefetchPoll(run.run_id);
      } else if (runsRunGraphId(run)) {
        runsStopDetailRefetchPoll();
      }
    } catch (err) { /* ignore */ }
  });

  runsStreamSse.addEventListener('progress', function (e) {
    runsRenderProgressItem(runsParseProgressLine(e.data));
  });

  runsStreamSse.addEventListener('state-change', function (e) {
    var rec;
    try { rec = JSON.parse(e.data); } catch (err) { return; }
    if (!rec || typeof rec !== 'object') { return; }
    var tsMs = typeof rec.ts_ms === 'number' && isFinite(rec.ts_ms) ? rec.ts_ms : 0;
    var kind = rec.kind ? String(rec.kind) : '';
    var toState = rec.to_state ? String(rec.to_state) : '';
    var msg = rec.message !== undefined && rec.message !== null ? String(rec.message) : '';
    var trimmedMsg = msg.trim();
    if (kind === 'agent' && !trimmedMsg) { return; }
    if (kind === 'activity' && toState === 'output') {
      if (!trimmedMsg || runsHasTextSummary(trimmedMsg)) { return; }
      runsAppendStreamLine('runs-stream-state', tsMs, 'output', msg, { outputText: trimmedMsg });
      return;
    }
    var kindLabel = '';
    var text = '';
    if (kind === 'agent') {
      kindLabel = toState ? runsStatusLabel(toState) : 'agent';
      text = msg;
    } else if (kind === 'activity') {
      kindLabel = toState || 'activity';
      text = msg;
    } else {
      kindLabel = kind || 'event';
      var parts = [];
      if (toState) { parts.push(runsStatusLabel(toState)); }
      if (trimmedMsg) { parts.push(msg); }
      text = parts.join(' \u00b7 ');
    }
    if (!kindLabel && !text.trim()) { return; }
    runsAppendStreamLine('runs-stream-state', tsMs, kindLabel, text);
  });

  runsStreamSse.addEventListener('end', function () {
    runsStopStream();
  });

  runsStreamSse.addEventListener('error', function (e) {
    if (e.target !== runsStreamSse) { return; }
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

function runsFindByGraphId(list, graphId) {
  if (!graphId || !Array.isArray(list)) { return null; }
  var target = String(graphId);
  for (var i = 0; i < list.length; i++) {
    if (runsRunGraphId(list[i]) === target) { return list[i]; }
  }
  return null;
}

// FR-2824: if `graphId` belongs to a headless run in Runs, open Runs and
// select that run instead of opening the standalone Graph view.
window.dmuxaiOpenRunForGraph = function (graphId, opts) {
  if (!graphId) { return Promise.resolve(false); }
  function openMatch(run) {
    if (!run) { return false; }
    if (typeof window.dmuxaiSetCenterView === 'function') {
      window.dmuxaiSetCenterView('runs', opts);
    }
    runsSelectRun(run.run_id);
    return true;
  }
  var cached = runsFindByGraphId(runsListData, graphId);
  if (cached) {
    return Promise.resolve(openMatch(cached));
  }
  return runsFetchJson('/api/runs').then(function (data) {
    runsListData = data && data.runs ? data.runs : [];
    runsIndicatorLastFetchAt = Date.now();
    return openMatch(runsFindByGraphId(runsListData, graphId));
  }).catch(function () {
    return false;
  });
};

function runsInit() {
  runsBindStreamScroll();
  var fullBtn = document.getElementById('runs-full-graph');
  if (fullBtn) {
    fullBtn.addEventListener('click', function () {
      if (runsDetailGraph && typeof window.dmuxaiOpenFullGraph === 'function') {
        window.dmuxaiOpenFullGraph(runsDetailGraph);
      }
    });
  }
  document.addEventListener('dmuxai:center-view', function () {
    runsSyncViewState();
  });
  document.addEventListener('dmuxai:state-change', function () {
    if (runsUiOpen) { runsOnDashboardStateChange(); }
    else { runsRefreshIndicator(); }
  });
  window.setTimeout(runsSyncViewState, 0);
  runsRefreshIndicator();
  runsIndicatorTimer = window.setInterval(runsRefreshIndicator, RUNS_INDICATOR_POLL_MS);
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', runsInit);
} else {
  runsInit();
}