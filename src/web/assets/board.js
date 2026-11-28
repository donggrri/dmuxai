'use strict';

/* Decision board dashboard (FR-3122..FR-3126). */

var BOARD_TOKEN_KEY = 'dmuxai.token';
var BOARD_LANES = [
  'auto', 'needs_decision', 'approved', 'in_progress', 'done', 'rejected', 'unknown'
];
var BOARD_MOVABLE_LANES = [
  'auto', 'needs_decision', 'in_progress', 'done', 'unknown'
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
var BOARD_ASK_POLL_MS = 3000;
var BOARD_ASK_TIMEOUT_MS = 15 * 60 * 1000;
var boardAskPollTimer = null;
var boardAskPollId = null;
var boardLastFocus = null;
var boardLastFocusItemId = null;
var boardNeedsFocus = false;
var boardSpecNotice = null;
var BOARD_SPEC_VERDICT_LABELS = {
  approved: 'Spec approved',
  changes_requested: 'Changes requested',
  rejected: 'Spec rejected',
  expired: 'Spec review expired',
  cancelled: 'Spec review cancelled'
};

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

function boardFetchJson(method, path, body, humanAction) {
  var tok = boardToken();
  var headers = { 'X-Dmuxai-Client': '1' };
  if (tok) {
    headers.Authorization = 'Bearer ' + tok;
  }
  if (humanAction === true) {
    headers['X-Dmuxai-Human'] = '1';
  }
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

/* SPEC-spec-review-gate FR-3714: a run paused for a human spec review. */
function boardSpecReviewPending(item) {
  return !!(item && item.spec_review && item.spec_review.status === 'pending');
}

/* FR-3715: a plan awaiting the owner's decision. */
function boardPlanReviewPending(item) {
  return !!(item && item.lane === 'needs_decision' && item.plan);
}

function boardReviewBadges(item) {
  var wrap = boardEl('div', 'board-review-badges');
  if (boardPlanReviewPending(item)) {
    wrap.appendChild(boardEl('span', 'board-review-badge board-review-badge-plan', 'Plan review'));
  }
  if (boardSpecReviewPending(item)) {
    wrap.appendChild(boardEl('span', 'board-review-badge board-review-badge-spec', 'Spec review'));
  }
  return wrap.childNodes.length ? wrap : null;
}

function boardSpecPathTarget(item, path) {
  var ws = item && item.workspace && item.workspace.path ? String(item.workspace.path) : '';
  if (!ws || path.charAt(0) === '/') { return path; }
  return ws.replace(/[/\\]+$/, '') + '/' + path;
}

function boardRenderSpecReview(drawer, item, draft) {
  var review = item.spec_review;
  if (!review) { return; }
  var section = boardEl('section', 'board-spec-review');
  section.id = 'board-spec-review';
  var title = boardEl('h4', 'board-review-title', 'Spec review (round ' + (review.round || 1) + ')');
  title.id = 'board-spec-review-title';
  section.appendChild(title);
  var list = boardEl('ul', 'board-spec-paths');
  var paths = review.spec_paths || [];
  for (var i = 0; i < paths.length; i++) {
    var li = document.createElement('li');
    var link = boardEl('a', 'md-path-link', paths[i]);
    link.href = '#';
    link.setAttribute('data-md-path', boardSpecPathTarget(item, paths[i]));
    li.appendChild(link);
    list.appendChild(li);
  }
  section.appendChild(list);
  if (review.status === 'pending') {
    var ta = document.createElement('textarea');
    ta.id = 'board-spec-feedback';
    ta.className = 'board-spec-feedback';
    ta.rows = 3;
    ta.placeholder = 'Feedback (required for Request changes)';
    ta.setAttribute('aria-label', 'Spec review feedback');
    if (draft) { ta.value = draft; }
    section.appendChild(ta);
    var actions = boardEl('div', 'board-spec-review-actions');
    [
      { id: 'board-spec-approve', label: 'Approve spec', verdict: 'approve', cls: 'btn btn-small btn-primary' },
      { id: 'board-spec-request-changes', label: 'Request changes', verdict: 'request_changes', cls: 'btn btn-small' },
      { id: 'board-spec-reject', label: 'Reject spec', verdict: 'reject', cls: 'btn btn-small btn-danger' }
    ].forEach(function (spec) {
      var btn = boardEl('button', spec.cls, spec.label);
      btn.type = 'button';
      btn.id = spec.id;
      btn.addEventListener('click', function () { boardSpecReviewDecide(item, spec.verdict); });
      actions.appendChild(btn);
    });
    section.appendChild(actions);
  } else {
    var verdict = boardEl('p', 'board-spec-verdict',
      (BOARD_SPEC_VERDICT_LABELS[review.status] || review.status) + ' (round ' + (review.round || 1) + ')');
    verdict.id = 'board-spec-verdict';
    section.appendChild(verdict);
  }
  var notice = boardEl('p', 'board-plan-notice board-spec-notice');
  notice.id = 'board-spec-notice';
  notice.setAttribute('aria-live', 'polite');
  if (boardSpecNotice && boardSpecNotice.id === item.id) {
    notice.textContent = boardSpecNotice.text;
  } else {
    notice.hidden = true;
  }
  section.appendChild(notice);
  drawer.appendChild(section);
}

function boardShowSpecNotice(id, text) {
  boardSpecNotice = { id: id, text: text };
  var notice = document.getElementById('board-spec-notice');
  if (notice && boardSelectedId === id) {
    notice.textContent = text;
    notice.hidden = false;
  }
}

function boardSpecReviewDecide(item, verdict) {
  var review = item && item.spec_review;
  if (!review) { return; }
  var body = { verdict: verdict, spec_digest: review.spec_digest || '' };
  var ta = document.getElementById('board-spec-feedback');
  var text = ta ? ta.value.trim() : '';
  if (verdict === 'request_changes' && !text) {
    boardShowSpecNotice(item.id, 'Request changes needs feedback.');
    if (ta) { try { ta.focus(); } catch (err) { /* ignore */ } }
    return;
  }
  if (text && verdict !== 'approve') { body.text = text; }
  if (verdict === 'reject' && !window.confirm('Reject this spec? The run stops.')) { return; }
  boardFetchJson('POST', '/api/board/' + encodeURIComponent(item.id) + '/spec-review', body, true)
    .then(function (updated) {
      boardSpecNotice = null;
      if (ta) { ta.value = ''; }
      if (updated && boardSelectedId === item.id) {
        boardSelectedItem = updated;
        boardRenderDrawer(updated);
      }
      boardRefreshList();
    })
    .catch(function (err) {
      var msg = (err && err.message) ? err.message : 'Spec review decision failed';
      if (err && err.status === 409 && /spec changed/i.test(msg)) {
        msg = 'The spec changed since it was shown \u2014 reload to review the new version.';
      } else if (err && err.status === 403) {
        msg = 'Refused (403): decide with a click in the dashboard.';
      }
      boardShowSpecNotice(item.id, msg);
    });
}

function boardUpdateBadge() {
  var toggle = document.getElementById('board-toggle');
  var badge = document.getElementById('board-badge');
  if (!toggle || !badge) { return; }
  var count = 0;
  for (var i = 0; i < boardItems.length; i++) {
    if (boardItems[i].lane === 'needs_decision' || boardSpecReviewPending(boardItems[i])) {
      count += 1;
    }
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
    var specPending = boardSpecReviewPending(item);
    if (item.lane !== 'needs_decision' && !specPending) { continue; }
    any = true;
    var card = boardEl('div', 'board-decision-card');
    card.setAttribute('tabindex', '0');
    card.setAttribute('data-item-id', item.id);
    card.appendChild(boardEl('div', 'board-decision-title', item.title));
    var myBadges = boardReviewBadges(item);
    if (myBadges) { card.appendChild(myBadges); }
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
    if (item.lane !== 'needs_decision') {
      var reviewBtn = boardEl('button', 'btn btn-small btn-primary', 'Review spec');
      reviewBtn.type = 'button';
      reviewBtn.setAttribute('data-board-action', 'open_spec_review');
      reviewBtn.setAttribute('data-item-id', item.id);
      actions.appendChild(reviewBtn);
      card.appendChild(actions);
      row.appendChild(card);
      continue;
    }
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

  var cutoffMs = Date.now() - 30 * 86400 * 1000;
  var toCheck = [];
  for (var ci = 0; ci < boardItems.length; ci++) {
    var cItem = boardItems[ci];
    if ((cItem.lane !== 'done' && cItem.lane !== 'unknown') || cItem.checked) { continue; }
    if (!cItem.updated_ms || cItem.updated_ms < cutoffMs) { continue; }
    if (boardWorkspaceFilter) {
      var cWs = cItem.workspace && cItem.workspace.path ? cItem.workspace.path : '';
      if (cWs.indexOf(boardWorkspaceFilter) < 0) { continue; }
    }
    toCheck.push(cItem);
  }
  toCheck.sort(function (a, b) {
    var diff = (a.updated_ms || 0) - (b.updated_ms || 0);
    if (diff !== 0) { return diff; }
    return String(a.id || '').localeCompare(String(b.id || ''));
  });
  if (toCheck.length) {
    var checkGroup = boardEl('div', 'board-to-check-group');
    checkGroup.id = 'board-to-check-group';
    checkGroup.appendChild(boardEl('h4', 'board-to-check-head', 'To check'));
    var checkRow = boardEl('div', 'board-my-row');
    for (var ti = 0; ti < toCheck.length; ti++) {
      var chk = toCheck[ti];
      var chkCard = boardEl('div', 'board-decision-card board-check-card');
      chkCard.setAttribute('tabindex', '0');
      chkCard.setAttribute('data-item-id', chk.id);
      chkCard.appendChild(boardEl('div', 'board-decision-title', chk.title));
      var chkMeta = [
        boardBasename(chk.workspace && chk.workspace.path),
        BOARD_LANE_LABELS[chk.lane] || chk.lane,
        boardAgeMs(chk.updated_ms)
      ].filter(Boolean).join(' · ');
      chkCard.appendChild(boardEl('div', 'board-decision-meta', chkMeta));
      var chkActions = boardEl('div', 'board-decision-actions');
      var chkBtn = boardEl('button', 'btn btn-small btn-primary', 'Mark checked');
      chkBtn.type = 'button';
      chkBtn.setAttribute('data-board-action', 'check');
      chkBtn.setAttribute('data-item-id', chk.id);
      chkActions.appendChild(chkBtn);
      chkCard.appendChild(chkActions);
      checkRow.appendChild(chkCard);
    }
    checkGroup.appendChild(checkRow);
    container.appendChild(checkGroup);
  }
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
      var badges = boardReviewBadges(item);
      if (badges) { card.appendChild(badges); }
      var menu = document.createElement('select');
      menu.className = 'board-card-menu';
      menu.setAttribute('aria-label', 'Move lane');
      menu.setAttribute('data-item-id', item.id);
      if (item.lane === 'approved' || item.lane === 'rejected') {
        var curOpt = document.createElement('option');
        curOpt.value = '';
        curOpt.textContent = BOARD_LANE_LABELS[item.lane] || item.lane;
        curOpt.disabled = true;
        curOpt.selected = true;
        menu.appendChild(curOpt);
      }
      for (var mi = 0; mi < BOARD_MOVABLE_LANES.length; mi++) {
        var opt = document.createElement('option');
        opt.value = BOARD_MOVABLE_LANES[mi];
        opt.textContent = BOARD_LANE_LABELS[BOARD_MOVABLE_LANES[mi]] || BOARD_MOVABLE_LANES[mi];
        if (BOARD_MOVABLE_LANES[mi] === item.lane) { opt.selected = true; }
        menu.appendChild(opt);
      }
      menu.addEventListener('change', function (ev) {
        var sel = ev.target;
        if (!sel.value) { return; }
        var id = sel.getAttribute('data-item-id');
        boardFetchJson('POST', '/api/board/' + encodeURIComponent(id) + '/lane', { lane: sel.value }, true)
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

function boardStopAskPoll(id) {
  if (id !== undefined && boardAskPollId !== id) { return; }
  if (boardAskPollTimer !== null) {
    window.clearInterval(boardAskPollTimer);
    boardAskPollTimer = null;
  }
  boardAskPollId = null;
}

function boardStartAskPoll(id) {
  boardStopAskPoll();
  boardAskPollId = id;
  var inFlight = false;
  boardAskPollTimer = window.setInterval(function () {
    var wrap = document.getElementById('board-drawer-wrap');
    if (!boardUiOpen || boardSelectedId !== id || !wrap || wrap.hidden || !wrap.classList.contains('open')) {
      boardStopAskPoll(id);
      return;
    }
    var pending = boardAskPending[id];
    if (!pending || pending.error) {
      boardStopAskPoll(id);
      return;
    }
    if (Date.now() - pending.startedAtMs >= BOARD_ASK_TIMEOUT_MS) {
      boardStopAskPoll(id);
      pending.error = 'No answer yet \u2014 check the thread later';
      if (boardSelectedItem && boardSelectedItem.id === id) {
        boardRenderDrawer(boardSelectedItem);
      } else {
        var curBusy = document.getElementById('board-ask-busy');
        if (curBusy) {
          curBusy.classList.add('board-ask-error');
          curBusy.hidden = false;
          curBusy.textContent = pending.error;
        }
      }
      return;
    }
    if (inFlight) { return; }
    inFlight = true;
    boardFetchJson('GET', '/api/board/' + encodeURIComponent(id)).then(function (item) {
      inFlight = false;
      if (boardSelectedId !== id) {
        boardStopAskPoll(id);
        return;
      }
      boardSelectedItem = item;
      if (boardCountPlannerMessages(item) > pending.baselineCount) {
        delete boardAskPending[id];
        boardStopAskPoll(id);
        boardRenderDrawer(item);
        boardRefreshList();
      } else if (Date.now() - pending.startedAtMs >= BOARD_ASK_TIMEOUT_MS) {
        boardStopAskPoll(id);
        pending.error = 'No answer yet \u2014 check the thread later';
        boardRenderDrawer(item);
      }
    }).catch(function () {
      inFlight = false;
    });
  }, BOARD_ASK_POLL_MS);
}

function boardFindCardForItem(id) {
  if (!id) { return null; }
  var cards = document.querySelectorAll('[data-item-id]');
  for (var i = 0; i < cards.length; i++) {
    if (cards[i].getAttribute('data-item-id') === id &&
        (cards[i].classList.contains('board-decision-card') || cards[i].classList.contains('board-card'))) {
      return cards[i];
    }
  }
  return null;
}

function boardCloseDrawer() {
  boardStopAskPoll();
  var wrap = document.getElementById('board-drawer-wrap');
  if (wrap) {
    wrap.classList.remove('open');
    wrap.hidden = true;
  }
  document.body.classList.remove('board-modal-open');
  var focusTarget = boardLastFocus;
  var fallbackId = boardLastFocusItemId;
  boardLastFocus = null;
  boardLastFocusItemId = null;
  boardNeedsFocus = false;
  boardSelectedId = null;
  boardSelectedItem = null;
  if (focusTarget && focusTarget.isConnected && typeof focusTarget.focus === 'function') {
    try { focusTarget.focus(); } catch (err) { /* ignore */ }
  } else if (fallbackId) {
    var card = boardFindCardForItem(fallbackId);
    if (card && typeof card.focus === 'function') {
      try { card.focus(); } catch (err) { /* ignore */ }
    }
  }
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
  var added = plan.spec_first_added || [];
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
      if (fields[fi] === 'job' && added.indexOf(step.id) >= 0) {
        tr.classList.add('board-plan-step-added');
        td.appendChild(boardEl('span', 'board-plan-step-note', 'added by spec-first'));
      }
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
  document.body.classList.add('board-modal-open');

  var existingInput = document.getElementById('board-chat-input');
  var prevItemId = boardSelectedItem ? boardSelectedItem.id : null;
  var preserveDraft = !!(existingInput && prevItemId === item.id);
  var draftText = preserveDraft ? existingInput.value : '';
  var existingNote = document.getElementById('board-chat-note-only');
  var draftNoteOnly = preserveDraft && existingNote ? !!existingNote.checked : false;
  var focusWasInput = preserveDraft && document.activeElement === existingInput;
  var existingSpecFeedback = document.getElementById('board-spec-feedback');
  var specDraft = existingSpecFeedback && prevItemId === item.id ? existingSpecFeedback.value : '';

  var pendingAsk = boardAskPending[item.id];
  if (pendingAsk && !pendingAsk.error) {
    if (boardCountPlannerMessages(item) > pendingAsk.baselineCount) {
      delete boardAskPending[item.id];
      boardStopAskPoll(item.id);
      pendingAsk = null;
    } else if (Date.now() - pendingAsk.startedAtMs >= BOARD_ASK_TIMEOUT_MS) {
      boardStopAskPoll(item.id);
      pendingAsk.error = 'No answer yet \u2014 check the thread later';
    } else if (boardUiOpen && boardSelectedId === item.id && boardAskPollId !== item.id) {
      boardStartAskPoll(item.id);
    }
  }
  drawer.textContent = '';
  var header = boardEl('div', 'board-modal-header');
  var titleEl = boardEl('h3', 'board-drawer-title', item.title);
  titleEl.id = 'board-drawer-title';
  header.appendChild(titleEl);
  var closeBtn = boardEl('button', 'btn sidebar-close board-modal-close', '\u00D7');
  closeBtn.type = 'button';
  closeBtn.id = 'board-modal-close';
  closeBtn.setAttribute('aria-label', 'Close');
  closeBtn.addEventListener('click', function (e) {
    e.stopPropagation();
    boardCloseDrawer();
  });
  header.appendChild(closeBtn);
  drawer.appendChild(header);
  var summaryEl = boardEl('p', 'board-drawer-summary', item.summary || '');
  drawer.appendChild(summaryEl);
  if (typeof window.dmuxaiLinkifyMdPaths === 'function') {
    window.dmuxaiLinkifyMdPaths(summaryEl);
  }
  boardRenderSpecReview(drawer, item, specDraft);
  var planReview = boardPlanReviewPending(item);
  if (item.plan) {
    if (planReview) {
      var planTitle = boardEl('h4', 'board-review-title', 'Plan review');
      planTitle.id = 'board-plan-review-title';
      drawer.appendChild(planTitle);
    }
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
    if (planReview) {
      var reviewActions = boardEl('div', 'board-plan-review-actions');
      reviewActions.setAttribute('aria-label', 'Plan review');
      var planApprove = boardEl('button', 'btn btn-small btn-primary', 'Approve');
      planApprove.type = 'button';
      planApprove.id = 'board-plan-approve';
      planApprove.addEventListener('click', function () { boardDecide(item.id, 'approve'); });
      reviewActions.appendChild(planApprove);
      var planReject = boardEl('button', 'btn btn-small btn-danger', 'Reject');
      planReject.type = 'button';
      planReject.id = 'board-plan-reject';
      planReject.addEventListener('click', function () {
        if (!window.confirm('Reject this item?')) { return; }
        boardDecide(item.id, 'reject');
      });
      reviewActions.appendChild(planReject);
      drawer.appendChild(reviewActions);
    }
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
  if ((item.lane === 'done' || item.lane === 'unknown') && !item.checked) {
    var checkWrap = boardEl('div', 'board-check-actions');
    var checkBtn = boardEl('button', 'btn btn-small btn-primary', 'Mark checked');
    checkBtn.type = 'button';
    checkBtn.id = 'board-mark-checked';
    checkBtn.addEventListener('click', function () { boardCheckItem(item.id); });
    checkWrap.appendChild(checkBtn);
    drawer.appendChild(checkWrap);
  } else if (item.checked) {
    var chkLine = 'Checked by ' + (item.checked.by || 'owner');
    if (item.checked.text) { chkLine += ' · ' + item.checked.text; }
    drawer.appendChild(boardEl('p', 'board-checked-record', chkLine));
  }
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
      var bubble = boardEl('div', 'board-chat-bubble', msg.text || '');
      line.appendChild(bubble);
      if (typeof window.dmuxaiLinkifyMdPaths === 'function') {
        window.dmuxaiLinkifyMdPaths(bubble);
      }
      thread.appendChild(line);
    }
  }
  chat.appendChild(thread);
  var ta = document.createElement('textarea');
  ta.id = 'board-chat-input';
  ta.className = 'board-chat-input';
  ta.rows = 3;
  ta.setAttribute('aria-label', 'Message');
  if (draftText) { ta.value = draftText; }
  chat.appendChild(ta);

  var chatActions = boardEl('div', 'board-chat-actions');
  var send = boardEl('button', 'btn btn-small btn-primary', 'Send');
  send.type = 'button';
  send.id = 'board-chat-send';
  chatActions.appendChild(send);

  var reviseBtn = boardEl('button', 'btn btn-small', 'Ask & revise plan');
  reviseBtn.type = 'button';
  reviseBtn.id = 'board-ask-revise';
  reviseBtn.hidden = !boardLaunchAllowed;
  chatActions.appendChild(reviseBtn);

  var noteLabel = boardEl('label', 'board-chat-note-only');
  noteLabel.hidden = !boardLaunchAllowed;
  var noteChk = document.createElement('input');
  noteChk.type = 'checkbox';
  noteChk.id = 'board-chat-note-only';
  noteChk.checked = draftNoteOnly;
  noteLabel.appendChild(noteChk);
  noteLabel.appendChild(document.createTextNode('Note only (no planner answer)'));
  chatActions.appendChild(noteLabel);
  chat.appendChild(chatActions);

  var isBusy = !!(boardLaunchAllowed && pendingAsk && !pendingAsk.error);
  if (isBusy) {
    send.disabled = true;
    reviseBtn.disabled = true;
  } else if (!item.plan) {
    reviseBtn.disabled = true;
  }

  var askBusy = boardEl('div', 'board-ask-busy');
  askBusy.id = 'board-ask-busy';
  askBusy.setAttribute('aria-live', 'polite');
  askBusy.setAttribute('role', 'status');
  if (!boardLaunchAllowed) {
    askBusy.hidden = true;
  } else if (pendingAsk && pendingAsk.error) {
    askBusy.hidden = false;
    askBusy.classList.add('board-ask-error');
    askBusy.textContent = pendingAsk.error;
  } else if (isBusy) {
    askBusy.hidden = false;
    askBusy.textContent = 'Asking planner\u2026';
  } else {
    askBusy.hidden = true;
  }
  chat.appendChild(askBusy);
  drawer.appendChild(chat);

  send.addEventListener('click', function () { boardSendChat(item.id); });
  reviseBtn.addEventListener('click', function () { boardAskPlanner(item.id, true); });
  ta.addEventListener('keydown', function (e) {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      boardSendChat(item.id);
    }
  });

  if (boardNeedsFocus) {
    boardNeedsFocus = false;
    if (typeof closeBtn.focus === 'function') {
      try { closeBtn.focus(); } catch (err) { /* ignore */ }
    }
  } else if (focusWasInput) {
    try { ta.focus(); } catch (err) { /* ignore */ }
  }
}

function boardAskPlanner(id, revise) {
  if (!boardLaunchAllowed) { return; }
  if (boardAskPending[id] && !boardAskPending[id].error) { return; }
  var ta = document.getElementById('board-chat-input');
  if (!ta || !ta.value.trim()) { return; }
  var text = ta.value.trim();
  var item = boardFindItem(id);
  var baselineCount = boardCountPlannerMessages(item);
  boardAskPending[id] = {
    baselineCount: baselineCount,
    error: null,
    startedAtMs: Date.now()
  };
  var busy = document.getElementById('board-ask-busy');
  var sendBtn = document.getElementById('board-chat-send');
  var reviseBtn = document.getElementById('board-ask-revise');
  if (sendBtn) { sendBtn.disabled = true; }
  if (reviseBtn) { reviseBtn.disabled = true; }
  if (busy) {
    busy.classList.remove('board-ask-error');
    busy.hidden = false;
    busy.textContent = 'Asking planner\u2026';
  }
  boardStartAskPoll(id);
  boardFetchJson('POST', '/api/board/' + encodeURIComponent(id) + '/ask', {
    text: text,
    revise: !!revise
  }).then(function () {
    var curTa = document.getElementById('board-chat-input');
    if (curTa) { curTa.value = ''; }
    if (boardSelectedId === id) {
      boardOpenDrawer(id, true);
    }
  }).catch(function (err) {
    boardStopAskPoll(id);
    var msg = (err && err.message) ? err.message : 'Ask failed';
    boardAskPending[id] = {
      baselineCount: baselineCount,
      error: msg,
      startedAtMs: Date.now()
    };
    var curBusy = document.getElementById('board-ask-busy');
    var curSend = document.getElementById('board-chat-send');
    var curRevise = document.getElementById('board-ask-revise');
    if (curSend) { curSend.disabled = false; }
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
  var noteChk = document.getElementById('board-chat-note-only');
  var noteOnly = !!(noteChk && noteChk.checked);
  if (boardLaunchAllowed && !noteOnly) {
    boardAskPlanner(id, false);
    return;
  }
  var text = ta.value.trim();
  boardFetchJson('POST', '/api/board/' + encodeURIComponent(id) + '/messages', { text: text })
    .then(function () {
      ta.value = '';
      boardOpenDrawer(id, true);
    })
    .catch(function () { /* ignore */ });
}

function boardOpenDrawer(id, skipPush) {
  var wrap = document.getElementById('board-drawer-wrap');
  var wasOpen = !!(wrap && !wrap.hidden && wrap.classList.contains('open'));
  if (!wasOpen) {
    boardLastFocus = document.activeElement;
    boardLastFocusItemId = id;
    boardNeedsFocus = true;
  }
  boardSelectedId = id;
  if (wrap) {
    wrap.hidden = false;
    wrap.classList.add('open');
  }
  document.body.classList.add('board-modal-open');
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

function boardCheckItem(id) {
  return boardFetchJson('POST', '/api/board/' + encodeURIComponent(id) + '/check', {}, true)
    .then(function () {
      boardRefreshList();
    })
    .catch(function () { /* ignore */ });
}

function boardShowDecisionNotice(autoRun) {
  var text = 'Approved';
  if (autoRun && autoRun.started) {
    text = 'Approved — run started';
  } else if (autoRun && autoRun.reason) {
    text = 'Approved — auto-run skipped: ' + autoRun.reason;
  }
  var el = document.getElementById('board-notice');
  if (el) {
    el.textContent = text;
    el.hidden = false;
  }
  if (typeof window.showMsg === 'function') {
    window.showMsg(text, false);
  }
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
  return boardFetchJson('POST', '/api/board/' + encodeURIComponent(id) + '/decision', body, true)
    .then(function (data) {
      boardPlanConflict = false;
      if (verdict === 'approve') {
        boardShowDecisionNotice(data && data.auto_run);
      }
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
    boardFetchJson('PUT', '/api/board/rules', body, true).then(function () {
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
  if (action === 'open_spec_review') {
    boardOpenDrawer(id);
    return true;
  }
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
  if (action === 'check') {
    boardCheckItem(id);
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
      e._boardInsideDrawer = true;
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
    if (e._boardInsideDrawer) { return; }
    var mdBackdrop = document.getElementById('md-viewer-backdrop');
    if (mdBackdrop && !mdBackdrop.hidden) { return; }
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
    var mdBackdrop = document.getElementById('md-viewer-backdrop');
    if (mdBackdrop && !mdBackdrop.hidden) { return; }
    var wrap = document.getElementById('board-drawer-wrap');
    if (wrap && !wrap.hidden && wrap.classList.contains('open')) {
      e.preventDefault();
      e.stopPropagation();
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