'use strict';

/* Decision board dashboard (FR-3122..FR-3126). */

var BOARD_TOKEN_KEY = 'dmuxai.token';
var BOARD_LANES = [
  'auto', 'needs_decision', 'approved', 'in_progress', 'done', 'rejected', 'unknown'
];
var BOARD_LANE_LABELS = {
  auto: 'Auto',
  needs_decision: 'Needs decision',
  approved: 'Approved',
  in_progress: 'In progress',
  done: 'Done',
  rejected: 'Rejected',
  unknown: 'Unknown'
};

var boardItems = [];
var boardSelectedId = null;
var boardSelectedItem = null;
var boardStream = null;
var boardStreamSince = 0;
var boardUiOpen = false;
var boardShowClosed = false;
var boardWorkspaceFilter = '';
var boardRulesOpen = false;
var boardPlanConflict = false;
var boardLaunchAllowed = false;
var boardAskPending = {};

function boardToken() {
  try { return window.sessionStorage.getItem(BOARD_TOKEN_KEY); } catch (err) { return null; }
}

function boardApiUrl(path) {
  if (typeof window.dmuxaiApiUrl === 'function') {
    return window.dmuxaiApiUrl(path);
  }
  return path;
}

function boardEl(tag, className, text) {
  var node = document.createElement(tag);
  if (className) { node.className = className; }
  if (text !== undefined && text !== null) {
    node.textContent = String(text);
  }
  return node;
}

function boardFetchJson(method, path, body) {
  var tok = boardToken();
  var headers = tok ? { Authorization: 'Bearer ' + tok } : {};
  var init = { method: method || 'GET', headers: headers, cache: 'no-store' };
  if (body !== undefined) {
    headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  return fetch(boardApiUrl(path), init).then(function (res) {
    return res.text().then(function (text) {
      var payload = null;
      if (text) {
        try { payload = JSON.parse(text); } catch (err) { payload = null; }
      }
      if (!res.ok) {
        var err = new Error((payload && payload.error) || ('HTTP ' + res.status));
        err.status = res.status;
        err.payload = payload;
        throw err;
      }
      return payload;
    });
  });
}

function boardBasename(path) {
  if (!path) { return ''; }
  var parts = String(path).split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

function boardAgeMs(ms) {
  if (!ms) { return ''; }
  var delta = Date.now() - ms;
  if (delta < 60000) { return 'just now'; }
  var mins = Math.floor(delta / 60000);
  if (mins < 60) { return mins + 'm'; }
  var hrs = Math.floor(mins / 60);
  if (hrs < 48) { return hrs + 'h'; }
  return Math.floor(hrs / 24) + 'd';
}

function boardUpdateBadge() {
  var toggle = document.getElementById('board-toggle');
  var badge = document.getElementById('board-badge');
  if (!toggle || !badge) { return; }
  var count = 0;
  for (var i = 0; i < boardItems.length; i++) {
    if (boardItems[i].lane === 'needs_decision') { count += 1; }
  }
  if (count > 0) {
    badge.hidden = false;
    badge.textContent = String(count);
    toggle.setAttribute('aria-label', 'Board, ' + count + ' need decision');
  } else {
    badge.hidden = true;
    badge.textContent = '';
    toggle.setAttribute('aria-label', 'Board');
  }
}

function boardFilteredItems() {
  var out = [];
  for (var i = 0; i < boardItems.length; i++) {
    var item = boardItems[i];
    if (!boardShowClosed && (item.lane === 'done' || item.lane === 'rejected')) {
      continue;
    }
    if (boardWorkspaceFilter) {
      var ws = item.workspace && item.workspace.path ? item.workspace.path : '';
      if (ws.indexOf(boardWorkspaceFilter) < 0) { continue; }
    }
    out.push(item);
  }
  return out;
}

function boardRenderMyDecisions(container, items) {
  container.textContent = '';
  var row = boardEl('div', 'board-my-row');
  var any = false;
  for (var i = 0; i < items.length; i++) {
    var item = items[i];
    if (item.lane !== 'needs_decision') { continue; }
    any = true;
    var card = boardEl('div', 'board-decision-card');
    card.setAttribute('tabindex', '0');
    card.setAttribute('data-item-id', item.id);
    card.appendChild(boardEl('div', 'board-decision-title', item.title));
    var meta = [
      boardBasename(item.workspace && item.workspace.path),
      item.kind,
      item.grade || '',
      item.source && item.source.bot ? item.source.bot : '',
      boardAgeMs(item.updated_ms)
    ].filter(Boolean).join(' · ');
    card.appendChild(boardEl('div', 'board-decision-meta', meta));
    if (item.recommendation) {
      card.appendChild(boardEl('div', 'board-decision-recommendation', item.recommendation));
    }
    var actions = boardEl('div', 'board-decision-actions');
    if (item.options && item.options.length) {
      for (var oi = 0; oi < item.options.length; oi++) {
        var optBtn = boardEl('button', 'btn btn-small', item.options[oi]);
        optBtn.type = 'button';
        optBtn.setAttribute('data-board-action', 'select_option');
        optBtn.setAttribute('data-option', String(oi));
        optBtn.setAttribute('data-item-id', item.id);
        optBtn.setAttribute('aria-pressed', 'false');
        actions.appendChild(optBtn);
      }
    }
    var approve = boardEl('button', 'btn btn-small btn-primary', 'Approve');
    approve.type = 'button';
    approve.setAttribute('data-board-action', 'approve');
    approve.setAttribute('data-item-id', item.id);
    actions.appendChild(approve);
    var changes = boardEl('button', 'btn btn-small', 'Request changes');
    changes.type = 'button';
    changes.setAttribute('data-board-action', 'request_changes');
    changes.setAttribute('data-item-id', item.id);
    actions.appendChild(changes);
    var reject = boardEl('button', 'btn btn-small btn-danger', 'Reject');
    reject.type = 'button';
    reject.setAttribute('data-board-action', 'reject');
    reject.setAttribute('data-item-id', item.id);
    actions.appendChild(reject);
    card.appendChild(actions);
    row.appendChild(card);
  }
  if (!any) {
    row.appendChild(boardEl('p', 'empty', 'No items need your decision'));
  }
  container.appendChild(row);
}

function boardRenderKanban(root, items) {
  var kanban = root.querySelector('#board-kanban');
  if (!kanban) { return; }
  kanban.textContent = '';
  for (var li = 0; li < BOARD_LANES.length; li++) {
    var laneId = BOARD_LANES[li];
    var lane = boardEl('div', 'board-lane board-lane-' + laneId);
    lane.id = 'board-lane-' + laneId;
    lane.appendChild(boardEl('div', 'board-lane-head', BOARD_LANE_LABELS[laneId] || laneId));
    var cards = boardEl('div', 'board-lane-cards');
    for (var i = 0; i < items.length; i++) {
      var item = items[i];
      if (item.lane !== laneId) { continue; }
      var card = boardEl('div', 'board-card');
      card.setAttribute('data-item-id', item.id);
      card.appendChild(boardEl('div', 'board-card-title', item.title));
      var sub = boardBasename(item.workspace && item.workspace.path) + ' · ' + item.kind;
      card.appendChild(boardEl('div', 'board-card-meta board-decision-meta', sub));
      var menu = document.createElement('select');
      menu.className = 'board-card-menu';
      menu.setAttribute('aria-label', 'Move lane');
      menu.setAttribute('data-item-id', item.id);
      for (var mi = 0; mi < BOARD_LANES.length; mi++) {
        var opt = document.createElement('option');
        opt.value = BOARD_LANES[mi];
        opt.textContent = BOARD_LANE_LABELS[BOARD_LANES[mi]] || BOARD_LANES[mi];
        if (BOARD_LANES[mi] === item.lane) { opt.selected = true; }
        menu.appendChild(opt);
      }
      menu.addEventListener('change', function (ev) {
        var sel = ev.target;
        var id = sel.getAttribute('data-item-id');
        boardFetchJson('POST', '/api/board/' + encodeURIComponent(id) + '/lane', { lane: sel.value })
          .then(function () { boardRefreshList(); })
          .catch(function () { /* ignore */ });
      });
      card.appendChild(menu);
      cards.appendChild(card);
    }
    lane.appendChild(cards);
    kanban.appendChild(lane);
  }
}

function boardRenderList() {
  var my = document.getElementById('board-my-decisions');
  var root = document.getElementById('board');
  if (!my || !root) { return; }
  var items = boardFilteredItems();
  boardRenderMyDecisions(my, items);
  boardRenderKanban(root, items);
  boardUpdateBadge();
}

function boardRefreshList() {
  var qs = boardShowClosed ? '?all=1' : '';
  return boardFetchJson('GET', '/api/board' + qs).then(function (data) {
    if (data && typeof data.launch_allowed === 'boolean') {
      boardLaunchAllowed = data.launch_allowed;
    }
    boardItems = data && data.items ? data.items : [];
    boardRenderList();
    if (boardSelectedId) {
      boardOpenDrawer(boardSelectedId, true);
    }
  });
}

function boardCloseStream() {
  if (boardStream) {
    boardStream.close();
    boardStream = null;
  }
}

function boardOpenStream() {
  boardCloseStream();
  if (!boardUiOpen) { return; }
  if (typeof window.dmuxaiOpenEventSource !== 'function') { return; }
  var path = '/api/board/events?since=' + encodeURIComponent(String(boardStreamSince));
  boardStream = window.dmuxaiOpenEventSource(path);
  if (!boardStream) { return; }
  boardStream.addEventListener('board', function (ev) {
    if (!ev.data) { return; }
    try {
      var payload = JSON.parse(ev.data);
      if (payload && typeof payload.seq === 'number') {
        boardStreamSince = Math.max(boardStreamSince, payload.seq);
      }
    } catch (err) { /* ignore */ }
    boardRefreshList();
  });
}

function boardSyncViewState() {
  var panel = document.getElementById('board');
  boardUiOpen = !!(panel && !panel.hidden);
  if (boardUiOpen) {
    boardRefreshList();
    boardOpenStream();
  } else {
    boardCloseStream();
    boardCloseDrawer();
  }
}

function boardCloseDrawer() {
  var wrap = document.getElementById('board-drawer-wrap');
  if (wrap) {
    wrap.classList.remove('open');
    wrap.hidden = true;
  }
  boardSelectedId = null;
  boardSelectedItem = null;
}

function boardPlanFieldInput(field, val) {
  if (field === 'job') {
    var area = document.createElement('textarea');
    area.className = 'board-plan-job';
    area.rows = 2;
    area.value = val == null ? '' : String(val);
    area.setAttribute('data-field', field);
    return area;
  }
  var input = document.createElement('input');
  input.type = 'text';
  input.value = val == null ? '' : String(val);
  input.setAttribute('data-field', field);
  return input;
}

function boardFormatChatTime(ms) {
  if (!ms) { return ''; }
  try {
    return new Date(ms).toLocaleString();
  } catch (err) {
    return '';
  }
}

function boardRenderPlanTable(tbody, plan) {
  tbody.textContent = '';
  if (!plan || !plan.steps) { return; }
  for (var i = 0; i < plan.steps.length; i++) {
    var step = plan.steps[i];
    var tr = document.createElement('tr');
    tr.setAttribute('data-step-index', String(i));
    var fields = ['id', 'role', 'runner', 'model', 'depends_on', 'job'];
    for (var fi = 0; fi < fields.length; fi++) {
      var td = document.createElement('td');
      var val = step[fields[fi]];
      if (fields[fi] === 'depends_on' && Array.isArray(val)) {
        val = val.join(',');
      }
      td.appendChild(boardPlanFieldInput(fields[fi], val));
      tr.appendChild(td);
    }
    var delTd = document.createElement('td');
    var delBtn = boardEl('button', 'btn btn-small', 'Remove');
    delBtn.type = 'button';
    delBtn.setAttribute('data-board-remove-step', String(i));
    delTd.appendChild(delBtn);
    tr.appendChild(delTd);
    tbody.appendChild(tr);
  }
}

function boardCountPlannerMessages(item) {
  if (!item || !item.thread) { return 0; }
  var count = 0;
  for (var i = 0; i < item.thread.length; i++) {
    if (item.thread[i] && item.thread[i].from === 'planner') {
      count += 1;
    }
  }
  return count;
}

function boardIsItemRunnable(item) {
  if (!boardLaunchAllowed || !item || !item.plan || !item.plan.hash) {
    return false;
  }
  if (item.lane === 'auto') {
    return true;
  }
  if (item.lane === 'approved' && item.decision && item.decision.plan_hash === item.plan.hash) {
    return true;
  }
  return false;
}

function boardRenderDrawer(item) {
  var wrap = document.getElementById('board-drawer-wrap');
  var drawer = document.getElementById('board-drawer');
  if (!wrap || !drawer) { return; }
  wrap.hidden = false;
  wrap.classList.add('open');
  var pendingAsk = boardAskPending[item.id];
  if (pendingAsk && !pendingAsk.error && boardCountPlannerMessages(item) > pendingAsk.baselineCount) {
    delete boardAskPending[item.id];
    pendingAsk = null;
  }
  drawer.textContent = '';
  drawer.appendChild(boardEl('h3', 'board-drawer-title', item.title));
  drawer.appendChild(boardEl('p', 'board-drawer-summary', item.summary || ''));
  if (item.plan) {
    drawer.appendChild(boardEl('p', 'board-plan-meta',
      (item.plan.hash || '—') + ' · revision ' + item.plan.revision));
    if (boardPlanConflict) {
      drawer.appendChild(boardEl('p', 'board-plan-notice', 'Plan changed — reload'));
    }
    var planScroll = boardEl('div', 'board-plan-table-wrap');
    var table = document.createElement('table');
    table.className = 'board-plan-table';
    table.setAttribute('id', 'board-plan-steps');
    var colgroup = document.createElement('colgroup');
    [
      { w: '90px', cls: 'board-plan-col-id' },
      { w: '110px', cls: 'board-plan-col-role' },
      { w: '110px', cls: 'board-plan-col-runner' },
      { w: '150px', cls: 'board-plan-col-model' },
      { w: '120px', cls: 'board-plan-col-deps' },
      { w: 'auto', cls: 'board-plan-col-job' },
      { w: '80px', cls: 'board-plan-col-actions' }
    ].forEach(function (col) {
      var cg = document.createElement('col');
      cg.className = col.cls;
      if (col.w !== 'auto') { cg.style.width = col.w; }
      colgroup.appendChild(cg);
    });
    table.appendChild(colgroup);
    var thead = document.createElement('thead');
    var hrow = document.createElement('tr');
    ['id', 'role', 'runner', 'model', 'depends_on', 'job', ''].forEach(function (h) {
      var th = document.createElement('th');
      th.textContent = h;
      hrow.appendChild(th);
    });
    thead.appendChild(hrow);
    table.appendChild(thead);
    var tbody = document.createElement('tbody');
    boardRenderPlanTable(tbody, item.plan);
    table.appendChild(tbody);
    planScroll.appendChild(table);
    drawer.appendChild(planScroll);
    var planActions = boardEl('div', 'board-plan-actions');
    var addStep = boardEl('button', 'btn btn-small', 'Add step');
    addStep.type = 'button';
    addStep.id = 'board-add-step';
    planActions.appendChild(addStep);
    var savePlan = boardEl('button', 'btn btn-small btn-primary', 'Save plan');
    savePlan.type = 'button';
    savePlan.id = 'board-save-plan';
    planActions.appendChild(savePlan);
    drawer.appendChild(planActions);
    savePlan.addEventListener('click', function () { boardSavePlan(item.id); });
    addStep.addEventListener('click', function () { boardAddPlanStep(); });
    var errBox = boardEl('div', 'board-plan-errors');
    errBox.id = 'board-plan-errors';
    drawer.appendChild(errBox);
    if (item.plan.previous_steps && item.plan.previous_steps.length) {
      var diff = boardEl('details', 'board-plan-diff');
      var sum = document.createElement('summary');
      sum.textContent = 'Diff vs previous revision';
      diff.appendChild(sum);
      var pre = boardEl('pre', 'board-plan-diff-body');
      var lines = [];
      for (var di = 0; di < item.plan.previous_steps.length; di++) {
        var prev = item.plan.previous_steps[di];
        var cur = item.plan.steps[di];
        if (!cur || prev.job !== cur.job) {
          lines.push((prev.id || di) + ': ' + (prev.job || '') + ' → ' + (cur ? cur.job : '(removed)'));
        }
      }
      pre.textContent = lines.join('\n');
      diff.appendChild(pre);
      drawer.appendChild(diff);
    }
  }
  var decisionLine = 'No decision yet';
  if (item.decision) {
    var dParts = [item.decision.verdict, item.decision.by ? 'by ' + item.decision.by : ''];
    if (item.decision.text) {
      dParts.push(item.decision.text);
    } else if (
      item.decision.option !== undefined &&
      item.decision.option !== null &&
      item.options &&
      item.options[item.decision.option]
    ) {
      dParts.push(item.options[item.decision.option]);
    }
    decisionLine = dParts.filter(Boolean).join(' · ');
  }
  drawer.appendChild(boardEl('p', 'board-decision-record', decisionLine));
  var runWrap = boardEl('div', 'board-run-actions');
  var runnable = boardIsItemRunnable(item);
  runWrap.hidden = !runnable;
  var runBtn = boardEl('button', 'btn btn-small btn-primary', 'Run');
  runBtn.type = 'button';
  runBtn.id = 'board-run';
  runBtn.hidden = !runnable;
  runWrap.appendChild(runBtn);
  var runErr = boardEl('div', 'board-run-error');
  runErr.id = 'board-run-error';
  runErr.setAttribute('aria-live', 'polite');
  runWrap.appendChild(runErr);
  drawer.appendChild(runWrap);
  runBtn.addEventListener('click', function () { boardRunItem(item); });

  var chat = boardEl('section', 'board-chat');
  chat.setAttribute('aria-label', 'Side chat');
  var thread = boardEl('div', 'board-chat-thread');
  thread.id = 'board-chat-thread';
  if (item.thread) {
    for (var ti = 0; ti < item.thread.length; ti++) {
      var msg = item.thread[ti];
      var line = boardEl('div', 'board-chat-msg');
      if (msg.from === 'owner') {
        line.classList.add('board-chat-msg-owner');
      }
      line.appendChild(boardEl('div', 'board-chat-sender', msg.from || 'unknown'));
      line.appendChild(boardEl('div', 'board-chat-time', boardFormatChatTime(msg.at_ms)));
      line.appendChild(boardEl('div', 'board-chat-bubble', msg.text || ''));
      thread.appendChild(line);
    }
  }
  chat.appendChild(thread);
  var ta = document.createElement('textarea');
  ta.id = 'board-chat-input';
  ta.rows = 3;
  ta.setAttribute('aria-label', 'Message');
  chat.appendChild(ta);
  var send = boardEl('button', 'btn btn-small btn-primary', 'Send');
  send.type = 'button';
  send.id = 'board-chat-send';
  chat.appendChild(send);
  drawer.appendChild(chat);

  send.addEventListener('click', function () { boardSendChat(item.id); });
  ta.addEventListener('keydown', function (e) {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      boardSendChat(item.id);
    }
  });

  var askSec = boardEl('section', 'board-ask');
  askSec.id = 'board-ask-section';
  askSec.setAttribute('aria-label', 'Ask planner');
  askSec.hidden = !boardLaunchAllowed;
  var askLabel = boardEl('label', 'board-ask-label', 'Ask planner');
  askLabel.setAttribute('for', 'board-ask-text');
  askSec.appendChild(askLabel);
  var askTa = document.createElement('textarea');
  askTa.id = 'board-ask-text';
  askTa.className = 'board-ask-text';
  askTa.rows = 2;
  askTa.setAttribute('aria-label', 'Ask planner');
  askSec.appendChild(askTa);
  var askActions = boardEl('div', 'board-ask-actions');
  var askBtn = boardEl('button', 'btn btn-small', 'Ask');
  askBtn.type = 'button';
  askBtn.id = 'board-ask';
  var reviseBtn = boardEl('button', 'btn btn-small btn-primary', 'Ask & revise plan');
  reviseBtn.type = 'button';
  reviseBtn.id = 'board-ask-revise';
  if (!item.plan) { reviseBtn.disabled = true; }
  var isBusy = !!(pendingAsk && !pendingAsk.error);
  if (isBusy) {
    askBtn.disabled = true;
    reviseBtn.disabled = true;
  }
  askActions.appendChild(askBtn);
  askActions.appendChild(reviseBtn);
  askSec.appendChild(askActions);
  var askBusy = boardEl('div', 'board-ask-busy');
  askBusy.id = 'board-ask-busy';
  askBusy.setAttribute('aria-live', 'polite');
  askBusy.setAttribute('role', 'status');
  if (pendingAsk && pendingAsk.error) {
    askBusy.hidden = false;
    askBusy.classList.add('board-ask-error');
    askBusy.textContent = pendingAsk.error;
  } else if (isBusy) {
    askBusy.hidden = false;
    askBusy.textContent = 'Asking planner\u2026';
  } else {
    askBusy.hidden = true;
  }
  askSec.appendChild(askBusy);
  drawer.appendChild(askSec);
  askBtn.addEventListener('click', function () { boardAskPlanner(item.id, false); });
  reviseBtn.addEventListener('click', function () { boardAskPlanner(item.id, true); });
}

function boardAskPlanner(id, revise) {
  if (!boardLaunchAllowed) { return; }
  var ta = document.getElementById('board-ask-text');
  if (!ta || !ta.value.trim()) { return; }
  var text = ta.value.trim();
  var item = boardFindItem(id);
  var baselineCount = boardCountPlannerMessages(item);
  boardAskPending[id] = { baselineCount: baselineCount, error: null };
  var busy = document.getElementById('board-ask-busy');
  var askBtn = document.getElementById('board-ask');
  var reviseBtn = document.getElementById('board-ask-revise');
  if (askBtn) { askBtn.disabled = true; }
  if (reviseBtn) { reviseBtn.disabled = true; }
  if (busy) {
    busy.classList.remove('board-ask-error');
    busy.hidden = false;
    busy.textContent = 'Asking planner\u2026';
  }
  boardFetchJson('POST', '/api/board/' + encodeURIComponent(id) + '/ask', {
    text: text,
    revise: !!revise
  }).then(function () {
    var curTa = document.getElementById('board-ask-text');
    if (curTa) { curTa.value = ''; }
  }).catch(function (err) {
    var msg = (err && err.message) ? err.message : 'Ask failed';
    boardAskPending[id] = { baselineCount: baselineCount, error: msg };
    var curBusy = document.getElementById('board-ask-busy');
    var curAsk = document.getElementById('board-ask');
    var curRevise = document.getElementById('board-ask-revise');
    if (curAsk) { curAsk.disabled = false; }
    if (curRevise) {
      var curItem = boardFindItem(id);
      curRevise.disabled = !(curItem && curItem.plan);
    }
    if (curBusy) {
      curBusy.classList.add('board-ask-error');
      curBusy.hidden = false;
      curBusy.textContent = msg;
    }
  });
}

function boardRunItem(item) {
  if (!boardLaunchAllowed || !boardIsItemRunnable(item)) { return; }
  if (!window.confirm('Run this plan now?')) { return; }
  var errEl = document.getElementById('board-run-error');
  if (errEl) { errEl.textContent = ''; }
  var body = item.plan && item.plan.hash ? { plan_hash: item.plan.hash } : {};
  boardFetchJson('POST', '/api/board/' + encodeURIComponent(item.id) + '/run', body)
    .then(function () {
      boardRefreshList();
    })
    .catch(function (err) {
      if (errEl) {
        errEl.textContent = (err && err.message) ? err.message : 'Run failed';
      }
    });
}

function boardCollectSteps() {
  var tbody = document.querySelector('#board-plan-steps tbody');
  if (!tbody) { return []; }
  var steps = [];
  var rows = tbody.querySelectorAll('tr');
  for (var i = 0; i < rows.length; i++) {
    var row = rows[i];
    var step = { id: '', role: '', runner: '', model: null, depends_on: [], job: '', task: null };
    var inputs = row.querySelectorAll('input, textarea');
    for (var j = 0; j < inputs.length; j++) {
      var field = inputs[j].getAttribute('data-field');
      var val = inputs[j].value.trim();
      if (field === 'depends_on') {
        step.depends_on = val ? val.split(',').map(function (s) { return s.trim(); }).filter(Boolean) : [];
      } else if (field === 'model') {
        step.model = val || null;
      } else if (field) {
        step[field] = val;
      }
    }
    steps.push(step);
  }
  return steps;
}

function boardSavePlan(id) {
  var steps = boardCollectSteps();
  var errBox = document.getElementById('board-plan-errors');
  if (errBox) { errBox.textContent = ''; }
  boardFetchJson('POST', '/api/board/' + encodeURIComponent(id) + '/plan', { steps: steps })
    .then(function () {
      boardPlanConflict = false;
      boardOpenDrawer(id, true);
      boardRefreshList();
    })
  .catch(function (err) {
    if (errBox) {
      errBox.textContent = err.message || 'Plan save failed';
    }
  });
}

function boardAddPlanStep() {
  var tbody = document.querySelector('#board-plan-steps tbody');
  if (!tbody) { return; }
  var tr = document.createElement('tr');
  ['id', 'role', 'runner', 'model', 'depends_on', 'job'].forEach(function (field) {
    var td = document.createElement('td');
    td.appendChild(boardPlanFieldInput(field, ''));
    tr.appendChild(td);
  });
  var delTd = document.createElement('td');
  var delBtn = boardEl('button', 'btn btn-small', 'Remove');
  delBtn.type = 'button';
  delBtn.setAttribute('data-board-remove-step', 'new');
  delTd.appendChild(delBtn);
  tr.appendChild(delTd);
  tbody.appendChild(tr);
}

function boardRemovePlanStep(index) {
  var tbody = document.querySelector('#board-plan-steps tbody');
  if (!tbody) { return; }
  var row = tbody.querySelector('tr[data-step-index="' + index + '"]');
  if (row) { row.remove(); }
}

function boardSendChat(id) {
  var ta = document.getElementById('board-chat-input');
  if (!ta || !ta.value.trim()) { return; }
  var text = ta.value.trim();
  boardFetchJson('POST', '/api/board/' + encodeURIComponent(id) + '/messages', { text: text })
    .then(function () {
      ta.value = '';
      boardOpenDrawer(id, true);
    })
    .catch(function () { /* ignore */ });
}

function boardOpenDrawer(id, skipPush) {
  boardSelectedId = id;
  var wrap = document.getElementById('board-drawer-wrap');
  if (wrap) {
    wrap.hidden = false;
    wrap.classList.add('open');
  }
  var cached = !skipPush ? boardFindItem(id) : null;
  if (cached) {
    boardRenderDrawer(cached);
  } else if (!skipPush) {
    var drawer = document.getElementById('board-drawer');
    if (drawer) { drawer.textContent = ''; }
  }
  boardFetchJson('GET', '/api/board/' + encodeURIComponent(id)).then(function (item) {
    if (boardSelectedId !== id) { return; }
    boardSelectedItem = item;
    boardPlanConflict = false;
    boardRenderDrawer(item);
  }).catch(function () {
    if (boardSelectedId === id) {
      boardCloseDrawer();
    }
  });
}

function boardFindItem(id) {
  for (var i = 0; i < boardItems.length; i++) {
    if (boardItems[i].id === id) { return boardItems[i]; }
  }
  return boardSelectedItem && boardSelectedItem.id === id ? boardSelectedItem : null;
}

function boardDecide(id, verdict, extra) {
  extra = extra || {};
  var body = { verdict: verdict };
  if (extra.text) { body.text = extra.text; }
  if (extra.option !== undefined) { body.option = extra.option; }
  var src = boardFindItem(id);
  if (src && src.plan && src.plan.hash) {
    body.plan_hash = src.plan.hash;
  }
  return boardFetchJson('POST', '/api/board/' + encodeURIComponent(id) + '/decision', body)
    .then(function () {
      boardPlanConflict = false;
      boardRefreshList();
    })
    .catch(function (err) {
      if (err.status === 409) {
        boardPlanConflict = true;
        if (boardSelectedId === id) {
          boardOpenDrawer(id, true);
        }
      }
    });
}

function boardBindToolbar() {
  var closed = document.getElementById('board-show-closed');
  if (closed) {
    closed.addEventListener('change', function () {
      boardShowClosed = !!closed.checked;
      boardRenderList();
    });
  }
  var ws = document.getElementById('board-workspace-filter');
  if (ws) {
    ws.addEventListener('change', function () {
      boardWorkspaceFilter = ws.value || '';
      boardRenderList();
    });
  }
  var rulesToggle = document.getElementById('board-rules-toggle');
  var rulesPanel = document.getElementById('board-rules-panel');
  if (rulesToggle && rulesPanel) {
    rulesToggle.addEventListener('click', function () {
      boardRulesOpen = !boardRulesOpen;
      rulesPanel.hidden = !boardRulesOpen;
      if (boardRulesOpen) { boardLoadRules(rulesPanel); }
    });
  }
  var saveRules = document.getElementById('board-rules-save');
  if (saveRules) {
    saveRules.addEventListener('click', function () {
      boardSaveRules(rulesPanel);
    });
  }
}

function boardLoadRules(panel) {
  boardFetchJson('GET', '/api/board/rules').then(function (rules) {
    var area = panel.querySelector('#board-rules-json');
    if (area) {
      area.value = JSON.stringify(rules, null, 2);
    }
  }).catch(function () { /* ignore */ });
}

function boardSaveRules(panel) {
  var area = panel.querySelector('#board-rules-json');
  if (!area) { return; }
  try {
    var body = JSON.parse(area.value);
    boardFetchJson('PUT', '/api/board/rules', body).then(function () {
      area.classList.remove('board-rules-error');
    }).catch(function () {
      area.classList.add('board-rules-error');
    });
  } catch (err) {
    area.classList.add('board-rules-error');
  }
}

function boardEnsureAnswerSendForm(card, id) {
  if (!card) { return; }
  var form = card.querySelector('.board-answer-send-form');
  if (!form) {
    form = boardEl('div', 'board-answer-send-form');
    var ta = document.createElement('textarea');
    ta.className = 'board-answer-note-input';
    ta.rows = 2;
    ta.setAttribute('aria-label', 'Optional note with answer');
    ta.setAttribute('data-item-id', id);
    ta.placeholder = 'Optional note';
    form.appendChild(ta);
    var send = boardEl('button', 'btn btn-small btn-primary', 'Send answer');
    send.type = 'button';
    send.setAttribute('data-board-action', 'submit_answer');
    send.setAttribute('data-item-id', id);
    form.appendChild(send);
    card.appendChild(form);
  }
  form.hidden = false;
}

function boardClearAnswerSendForm(card) {
  if (!card) { return; }
  var form = card.querySelector('.board-answer-send-form');
  if (form) {
    form.hidden = true;
    var ta = form.querySelector('.board-answer-note-input');
    if (ta) { ta.value = ''; }
  }
}

function boardEnsureRequestChangesForm(card, id) {
  if (!card || card.querySelector('.board-request-changes-form')) { return; }
  var form = boardEl('div', 'board-request-changes-form');
  var ta = document.createElement('textarea');
  ta.className = 'board-request-changes-input';
  ta.rows = 3;
  ta.setAttribute('aria-label', 'Direction for changes');
  ta.setAttribute('data-item-id', id);
  form.appendChild(ta);
  var row = boardEl('div', 'board-decision-actions');
  var send = boardEl('button', 'btn btn-small btn-primary', 'Send');
  send.type = 'button';
  send.setAttribute('data-board-action', 'submit_changes');
  send.setAttribute('data-item-id', id);
  row.appendChild(send);
  var cancel = boardEl('button', 'btn btn-small', 'Cancel');
  cancel.type = 'button';
  cancel.setAttribute('data-board-action', 'cancel_changes');
  cancel.setAttribute('data-item-id', id);
  row.appendChild(cancel);
  form.appendChild(row);
  card.appendChild(form);
  ta.focus();
}

function boardOnActionClick(e) {
  var btn = e.target && e.target.closest ? e.target.closest('[data-board-action]') : null;
  if (!btn) { return false; }
  var action = btn.getAttribute('data-board-action');
  var id = btn.getAttribute('data-item-id');
  if (!action || !id) { return false; }
  e.stopPropagation();
  var card = btn.closest('.board-decision-card');
  if (action === 'request_changes') {
    boardEnsureRequestChangesForm(card, id);
    return true;
  }
  if (action === 'cancel_changes') {
    var form = card && card.querySelector('.board-request-changes-form');
    if (form) { form.remove(); }
    return true;
  }
  if (action === 'submit_changes') {
    var area = card && card.querySelector('.board-request-changes-input');
    if (area && area.value.trim()) {
      boardDecide(id, 'request_changes', { text: area.value.trim() });
    }
    return true;
  }
  if (action === 'select_option') {
    var optBtns = card ? card.querySelectorAll('[data-board-action="select_option"]') : [];
    for (var si = 0; si < optBtns.length; si++) {
      var selected = optBtns[si] === btn;
      optBtns[si].setAttribute('aria-pressed', selected ? 'true' : 'false');
      optBtns[si].classList.toggle('board-option-selected', selected);
    }
    boardEnsureAnswerSendForm(card, id);
    return true;
  }
  if (action === 'submit_answer') {
    var picked = card && card.querySelector('[data-board-action="select_option"][aria-pressed="true"]');
    if (!picked) { return true; }
    var answerOpt = parseInt(picked.getAttribute('data-option'), 10);
    var noteArea = card && card.querySelector('.board-answer-note-input');
    var answerText = noteArea && noteArea.value.trim()
      ? noteArea.value.trim()
      : picked.textContent;
    boardDecide(id, 'answer', { option: answerOpt, text: answerText });
    boardClearAnswerSendForm(card);
    return true;
  }
  if (action === 'approve') {
    boardDecide(id, 'approve');
    return true;
  }
  if (action === 'reject') {
    if (!window.confirm('Reject this item?')) { return true; }
    boardDecide(id, 'reject');
    return true;
  }
  return false;
}

function boardOnBodyClick(e) {
  if (boardOnActionClick(e)) { return; }
  var target = e.target;
  if (!target || !target.closest) { return; }
  if (target.closest('select, input, textarea, button, a')) { return; }
  var card = target.closest('.board-card, .board-decision-card');
  if (!card) { return; }
  var id = card.getAttribute('data-item-id');
  if (id) {
    boardOpenDrawer(id);
  }
}

function boardInit() {
  boardBindToolbar();
  var body = document.getElementById('board-body');
  if (body) {
    body.addEventListener('click', boardOnBodyClick);
  }
  var drawer = document.getElementById('board-drawer');
  if (drawer) {
    drawer.addEventListener('click', function (e) {
      var btn = e.target && e.target.closest ? e.target.closest('[data-board-remove-step]') : null;
      if (btn) {
        boardRemovePlanStep(btn.getAttribute('data-board-remove-step'));
      }
    });
  }
  var backdrop = document.getElementById('board-drawer-backdrop');
  if (backdrop) {
    backdrop.addEventListener('click', boardCloseDrawer);
  }
  document.addEventListener('click', function (e) {
    var wrap = document.getElementById('board-drawer-wrap');
    if (!wrap || wrap.hidden || !wrap.classList.contains('open')) { return; }
    var target = e.target;
    if (target && target.closest && target.closest('.board-card, .board-decision-card, #board-drawer')) {
      return;
    }
    boardCloseDrawer();
  });
  document.addEventListener('keydown', function (e) {
    if (e.key !== 'Escape') { return; }
    var wrap = document.getElementById('board-drawer-wrap');
    if (wrap && !wrap.hidden && wrap.classList.contains('open')) {
      boardCloseDrawer();
    }
  });
  document.addEventListener('dmuxai:center-view', function () {
    boardSyncViewState();
  });
  window.setTimeout(boardSyncViewState, 0);
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', boardInit);
} else {
  boardInit();
}