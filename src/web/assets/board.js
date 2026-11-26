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
      card.appendChild(boardEl('div', 'board-decision-meta', item.recommendation));
    }
    var actions = boardEl('div', 'board-decision-actions');
    var approve = boardEl('button', 'btn btn-small', 'Approve');
    approve.type = 'button';
    approve.setAttribute('data-board-action', 'approve');
    approve.setAttribute('data-item-id', item.id);
    actions.appendChild(approve);
    var changes = boardEl('button', 'btn btn-small', 'Request changes');
    changes.type = 'button';
    changes.setAttribute('data-board-action', 'request_changes');
    changes.setAttribute('data-item-id', item.id);
    actions.appendChild(changes);
    var reject = boardEl('button', 'btn btn-small', 'Reject');
    reject.type = 'button';
    reject.setAttribute('data-board-action', 'reject');
    reject.setAttribute('data-item-id', item.id);
    actions.appendChild(reject);
    if (item.options && item.options.length) {
      for (var oi = 0; oi < item.options.length; oi++) {
        var optBtn = boardEl('button', 'btn btn-small', item.options[oi]);
        optBtn.type = 'button';
        optBtn.setAttribute('data-board-action', 'answer');
        optBtn.setAttribute('data-option', String(oi));
        optBtn.setAttribute('data-item-id', item.id);
        actions.appendChild(optBtn);
      }
    }
    card.appendChild(actions);
    card.addEventListener('click', function (e) {
      if (e.target && e.target.getAttribute && e.target.getAttribute('data-board-action')) {
        return;
      }
      boardOpenDrawer(item.id);
    });
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
      card.addEventListener('click', function (e) {
        if (e.target && e.target.tagName === 'SELECT') { return; }
        boardOpenDrawer(item.id);
      });
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
      var input = document.createElement('input');
      input.type = 'text';
      var val = step[fields[fi]];
      if (fields[fi] === 'depends_on' && Array.isArray(val)) {
        val = val.join(',');
      }
      input.value = val == null ? '' : String(val);
      input.setAttribute('data-field', fields[fi]);
      td.appendChild(input);
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

function boardRenderDrawer(item) {
  var wrap = document.getElementById('board-drawer-wrap');
  var drawer = document.getElementById('board-drawer');
  if (!wrap || !drawer) { return; }
  wrap.hidden = false;
  wrap.classList.add('open');
  drawer.textContent = '';
  drawer.appendChild(boardEl('h3', 'board-drawer-title', item.title));
  drawer.appendChild(boardEl('p', 'board-drawer-summary', item.summary || ''));
  if (item.plan) {
    drawer.appendChild(boardEl('p', 'board-plan-meta',
      'Plan rev ' + item.plan.revision + ' · ' + (item.plan.hash || '')));
    if (boardPlanConflict) {
      drawer.appendChild(boardEl('p', 'board-plan-notice', 'Plan changed — reload'));
    }
    var table = document.createElement('table');
    table.className = 'board-plan-table';
    table.setAttribute('id', 'board-plan-steps');
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
    drawer.appendChild(table);
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
  if (item.decision) {
    drawer.appendChild(boardEl('p', 'board-decision-record',
      'Last decision: ' + item.decision.verdict + ' by ' + item.decision.by));
  }
  var chat = boardEl('section', 'board-chat');
  chat.setAttribute('aria-label', 'Side chat');
  var thread = boardEl('div', 'board-chat-thread');
  thread.id = 'board-chat-thread';
  if (item.thread) {
    for (var ti = 0; ti < item.thread.length; ti++) {
      var msg = item.thread[ti];
      var line = boardEl('div', 'board-chat-msg',
        msg.from + ' · ' + boardAgeMs(msg.at_ms) + '\n' + msg.text);
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

  savePlan.addEventListener('click', function () { boardSavePlan(item.id); });
  addStep.addEventListener('click', function () { boardAddPlanStep(); });
  send.addEventListener('click', function () { boardSendChat(item.id); });
  ta.addEventListener('keydown', function (e) {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      boardSendChat(item.id);
    }
  });
  drawer.addEventListener('click', function (e) {
    var btn = e.target;
    if (btn && btn.getAttribute && btn.getAttribute('data-board-remove-step') !== null) {
      boardRemovePlanStep(btn.getAttribute('data-board-remove-step'));
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
    var inputs = row.querySelectorAll('input');
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
    var input = document.createElement('input');
    input.type = 'text';
    input.setAttribute('data-field', field);
    td.appendChild(input);
    tr.appendChild(td);
  });
  tr.appendChild(document.createElement('td'));
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
  boardFetchJson('GET', '/api/board/' + encodeURIComponent(id)).then(function (item) {
    boardSelectedItem = item;
    boardPlanConflict = false;
    boardRenderDrawer(item);
  }).catch(function () { boardCloseDrawer(); });
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

function boardOnActionClick(e) {
  var btn = e.target;
  if (!btn || !btn.getAttribute) { return; }
  var action = btn.getAttribute('data-board-action');
  if (!action) { return; }
  var id = btn.getAttribute('data-item-id');
  if (!id) { return; }
  e.stopPropagation();
  boardOpenDrawer(id);
  if (action === 'approve') {
    boardDecide(id, 'approve');
  } else if (action === 'reject') {
    boardDecide(id, 'reject');
  } else if (action === 'request_changes') {
    var text = window.prompt('Direction for changes');
    if (text) { boardDecide(id, 'request_changes', { text: text }); }
  } else if (action === 'answer') {
    var opt = parseInt(btn.getAttribute('data-option'), 10);
    boardDecide(id, 'answer', { option: opt, text: btn.textContent });
  }
}

function boardInit() {
  boardBindToolbar();
  var my = document.getElementById('board-my-decisions');
  if (my) {
    my.addEventListener('click', boardOnActionClick);
  }
  var backdrop = document.getElementById('board-drawer-backdrop');
  if (backdrop) {
    backdrop.addEventListener('click', boardCloseDrawer);
  }
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