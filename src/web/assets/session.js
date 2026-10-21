'use strict';
// SPEC-dashboard-parity FR-780..FR-784, UX-780: chat session, pickers,
// command palette and lifecycle controls for `dmuxai --web`. Self-contained:
// it shares only the token in sessionStorage, the <section id="session">
// container and app.js's `dmuxai:snapshot` DOM event (one SSE connection).

(function () {
  var TOKEN_KEY = 'dmuxai.token';
  var PROMPT_ROUTES = [
    { value: 'auto', label: 'Auto' },
    { value: 'plan', label: 'Plan' },
    { value: 'direct', label: 'Direct' }
  ];
  var SHARE_TARGETS = ['codex', 'cursor', 'agy'];

  var SESSIONS_OPEN_PREFIX = 'dmuxai.sessions.open.';

  function storageGet(key) {
    try { return window.sessionStorage.getItem(key); } catch (err) { return null; }
  }

  function storageSet(key, value) {
    try { window.sessionStorage.setItem(key, value); } catch (err) { /* ignore */ }
  }

  function localGet(key) {
    try { return window.localStorage.getItem(key); } catch (err) { return null; }
  }

  function localSet(key, value) {
    try { window.localStorage.setItem(key, value); } catch (err) { /* ignore */ }
  }

  var token = (function () {
    var fromQuery = new URLSearchParams(location.search).get('token');
    if (fromQuery) {
      storageSet(TOKEN_KEY, fromQuery);
      return fromQuery;
    }
    return storageGet(TOKEN_KEY);
  })();

  var root = null;
  var ui = {};
  var session = null;
  var viewOnly = false;
  var pending = null;
  var conversationShown = null;
  var messageNodes = {};
  var stickToBottom = true;
  // Last full dashboard snapshot (graph, agents) for the inline activity rows.
  var lastSnap = null;
  // Expanded state of inline rows, keyed by data-aux-key (default collapsed).
  var auxExpanded = {};
  var palette = null;
  var agentPopover = null;
  var workspaceKey = '';
  var workspaceInitialized = null;
  var sessionsOpen = false;
  var sessionsFilter = '';
  // A toggle made before the first snapshot names the workspace is kept here
  // and saved under the real workspace key once it is known.
  var pendingSessionsOpen = null;

  function sessionsStorageKey() {
    return SESSIONS_OPEN_PREFIX + (workspaceKey || 'default');
  }

  function syncSessionsBackdrop() {
    if (!ui.sessionsBackdrop) { return; }
    var narrow = window.innerWidth <= 900;
    ui.sessionsBackdrop.classList.toggle('active', sessionsOpen && narrow);
  }

  function setSessionsOpen(open, persist) {
    sessionsOpen = !!open;
    if (ui.sessionsSidebar) {
      ui.sessionsSidebar.classList.toggle('open', sessionsOpen);
      if (sessionsOpen) {
        ui.sessionsSidebar.removeAttribute('inert');
      } else {
        ui.sessionsSidebar.setAttribute('inert', '');
      }
    }
    if (ui.sessionsToggle) {
      ui.sessionsToggle.classList.toggle('active', sessionsOpen);
      ui.sessionsToggle.setAttribute('aria-expanded', sessionsOpen ? 'true' : 'false');
    }
    document.body.classList.toggle('left-sidebar-open', sessionsOpen);
    syncSessionsBackdrop();
    if (persist) {
      if (workspaceInitialized === 'default') {
        pendingSessionsOpen = sessionsOpen;
      } else {
        localSet(sessionsStorageKey(), sessionsOpen ? '1' : '0');
      }
    }
  }

  function initSessionsSidebarForWorkspace(ws) {
    var key = ws || 'default';
    if (workspaceInitialized === key) { return; }
    workspaceKey = key;
    workspaceInitialized = key;
    if (key !== 'default' && pendingSessionsOpen !== null) {
      var pending = pendingSessionsOpen;
      pendingSessionsOpen = null;
      setSessionsOpen(pending, true);
      return;
    }
    var saved = localGet(sessionsStorageKey());
    if (saved === '1') {
      setSessionsOpen(true, false);
    } else if (saved === '0') {
      setSessionsOpen(false, false);
    } else {
      setSessionsOpen(window.innerWidth > 1200, false);
    }
  }

  function relativeTime(ms) {
    var ts = Number(ms) || 0;
    if (!ts) { return ''; }
    var diffSec = Math.max(0, Math.floor((Date.now() - ts) / 1000));
    if (diffSec < 60) { return diffSec + 's ago'; }
    var diffMin = Math.floor(diffSec / 60);
    if (diffMin < 60) { return diffMin + 'm ago'; }
    var diffHr = Math.floor(diffMin / 60);
    if (diffHr < 24) { return diffHr + 'h ago'; }
    var diffDay = Math.floor(diffHr / 24);
    return diffDay + 'd ago';
  }

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) { node.className = className; }
    if (text !== undefined && text !== null) { node.textContent = text; }
    return node;
  }

  function button(label, parity, onClick, extraClass) {
    var b = el('button', 'btn', label);
    b.type = 'button';
    if (extraClass) {
      extraClass.split(/\s+/).forEach(function (cls) {
        if (cls) { b.classList.add(cls); }
      });
    }
    if (parity) { b.setAttribute('data-parity', parity); }
    b.addEventListener('click', onClick);
    return b;
  }

  function api(path, body) {
    return fetch(path, {
      method: 'POST',
      headers: {
        Authorization: 'Bearer ' + token,
        'Content-Type': 'application/json'
      },
      cache: 'no-store',
      body: JSON.stringify(body)
    }).then(function (resp) {
      return resp.text().then(function (text) {
        var data = null;
        try { data = text ? JSON.parse(text) : null; } catch (err) { data = null; }
        if (!resp.ok) {
          var message = (data && data.error) ? data.error : ('HTTP ' + resp.status);
          throw new Error(message);
        }
        return data;
      });
    });
  }

  function notify(text, isError) {
    ui.notice.textContent = text || '';
    ui.notice.hidden = !text;
    ui.notice.className = 'session-notice' + (isError ? ' session-notice-error' : '');
  }

  function command(body) {
    if (viewOnly) { return Promise.resolve(null); }
    notify('');
    return api('/api/command', body).then(function (data) {
      if (data && data.session) { render(data.session); }
      return data;
    }).catch(function (err) {
      notify(err.message, true);
      return null;
    });
  }

  function sendPrompt(text) {
    if (viewOnly || !text.trim()) { return Promise.resolve(false); }
    notify('');
    var body = { text: text };
    if (session && session.selected_agent) { body.agent = session.selected_agent; }
    if (ui.route && ui.route.value && ui.route.value !== 'auto') {
      body.route = ui.route.value;
    }
    return api('/api/prompt', body).then(function () {
      stickToBottom = true;
      return true;
    }).catch(function (err) {
      notify(err.message, true);
      return false;
    });
  }

  function selectedAgent() {
    if (!session) { return null; }
    for (var i = 0; i < session.agents.length; i++) {
      if (session.agents[i].id === session.selected_agent) { return session.agents[i]; }
    }
    return null;
  }

  function agentColor(id) {
    var snap = window.__dmuxSessionAgents || {};
    return snap[id] || null;
  }

  // ---- layout -----------------------------------------------------------

  function bindSessionsSidebar() {
    ui.sessionsSidebar = document.getElementById('sessions-sidebar');
    ui.sessionsToggle = document.getElementById('sessions-toggle');
    ui.sessionsNew = document.getElementById('sessions-new');
    ui.sessionsClose = document.getElementById('sessions-close');
    ui.sessionsFilter = document.getElementById('sessions-filter');
    ui.sessionsViewOnly = document.getElementById('sessions-view-only');
    ui.sessionsList = document.getElementById('sessions-list');
    ui.sessionsBackdrop = document.getElementById('sessions-backdrop');

    if (ui.sessionsToggle) {
      ui.sessionsToggle.addEventListener('click', function () {
        setSessionsOpen(!sessionsOpen, true);
      });
    }
    if (ui.sessionsClose) {
      ui.sessionsClose.addEventListener('click', function () {
        setSessionsOpen(false, true);
      });
    }
    if (ui.sessionsBackdrop) {
      ui.sessionsBackdrop.addEventListener('click', function () {
        setSessionsOpen(false, true);
      });
    }
    if (ui.sessionsNew) {
      ui.sessionsNew.addEventListener('click', function () {
        command({ command: 'new_conversation' });
      });
    }
    window.addEventListener('dmuxai:workspace-info', function () { renderConversations(); });
    if (ui.sessionsFilter) {
      ui.sessionsFilter.addEventListener('input', function () {
        sessionsFilter = ui.sessionsFilter.value || '';
        renderConversations();
      });
    }
    window.addEventListener('resize', syncSessionsBackdrop);
    initSessionsSidebarForWorkspace('default');
  }

  function build() {
    root.textContent = '';
    root.classList.add('session');
    bindSessionsSidebar();

    var head = el('div', 'session-head');
    head.appendChild(el('h2', 'panel-title', 'Chat'));
    ui.title = el('span', 'session-title');
    head.appendChild(ui.title);
    root.appendChild(head);

    ui.viewOnly = el('p', 'hint session-view-only',
      'View-only: start dmuxai --web to chat from the browser.');
    ui.viewOnly.hidden = true;
    root.appendChild(ui.viewOnly);

    var bar = el('div', 'session-bar');
    var actions = el('div', 'session-actions');
    ui.cancel = button('Cancel', 'cancel', function () {
      command({ command: 'cancel', agent: session.selected_agent });
    }, 'btn-danger');
    ui.newConv = button('New', 'new', function () { command({ command: 'new_conversation' }); });
    ui.status = button('Status', 'status', function () { command({ command: 'status' }); });
    ui.details = button('Details', 'details', function () { command({ command: 'toggle_details' }); });
    ui.share = button('Share', 'share', function () { openShare(); });
    ui.palette = button('Commands (Ctrl+K)', null, function () { openPalette(''); });
    ui.quit = button('Quit', 'quit', function () {
      if (window.confirm('Quit dmuxai? This stops the TUI and the dashboard.')) {
        command({ command: 'quit' });
      }
    });
    [ui.cancel, ui.newConv, ui.status, ui.details, ui.share, ui.palette, ui.quit].forEach(function (b) {
      actions.appendChild(b);
    });
    bar.appendChild(actions);
    root.appendChild(bar);

    var body = el('div', 'session-body');
    ui.transcript = el('ol', 'session-transcript');
    ui.transcript.setAttribute('aria-live', 'polite');
    ui.transcript.addEventListener('scroll', function () {
      var t = ui.transcript;
      stickToBottom = t.scrollHeight - t.scrollTop - t.clientHeight < 40;
    });
    body.appendChild(ui.transcript);
    root.appendChild(body);

    ui.notice = el('p', 'session-notice');
    ui.notice.hidden = true;
    ui.notice.setAttribute('role', 'status');
    root.appendChild(ui.notice);

    var form = el('div', 'session-prompt');
    var composerTop = el('div', 'session-composer-top');
    ui.agentPicker = el('button', 'session-agent-picker');
    ui.agentPicker.type = 'button';
    ui.agentPicker.setAttribute('data-parity', 'agent');
    ui.agentPicker.setAttribute('aria-haspopup', 'dialog');
    ui.agentPicker.setAttribute('aria-expanded', 'false');
    ui.agentPicker.addEventListener('click', function (e) {
      e.preventDefault();
      if (viewOnly) { return; }
      if (agentPopover) { closeAgentPopover(); } else { openAgentPopover(); }
    });
    composerTop.appendChild(ui.agentPicker);
    ui.input = el('textarea', 'session-input');
    ui.input.rows = 1;
    ui.input.placeholder = 'Message the selected agent. Enter sends, Shift+Enter adds a line, / opens commands.';
    ui.input.setAttribute('data-parity', 'prompt-newline');
    ui.input.setAttribute('aria-label', 'Prompt');
    ui.input.addEventListener('keydown', onPromptKey);
    ui.input.addEventListener('input', function () {
      if (ui.input.value === '/') {
        ui.input.value = '';
        openPalette('', true);
      }
      autoGrowInput();
    });
    composerTop.appendChild(ui.input);
    form.appendChild(composerTop);
    var sendCol = el('div', 'session-send-col');
    ui.route = picker('Route', 'route', function () { /* value read on send */ });
    setOptions(ui.route, PROMPT_ROUTES, 'auto');
    sendCol.appendChild(ui.route.parentNode);
    ui.send = button('Send', 'prompt-send', submitPrompt, 'btn-primary');
    sendCol.appendChild(ui.send);
    form.appendChild(sendCol);

    root.appendChild(form);
    var parityModel = el('span', 'session-parity-anchor');
    parityModel.hidden = true;
    parityModel.setAttribute('data-parity', 'model');
    root.appendChild(parityModel);
    var parityEffort = el('span', 'session-parity-anchor');
    parityEffort.hidden = true;
    parityEffort.setAttribute('data-parity', 'effort');
    root.appendChild(parityEffort);
    trackHeaderHeight();
    window.setInterval(tickLiveRows, 1000);

    document.addEventListener('keydown', function (e) {
      if ((e.ctrlKey || e.metaKey) && (e.key === 'k' || e.key === 'K')) {
        e.preventDefault();
        openPalette('');
        return;
      }
      if (e.key === 'F3') {
        e.preventDefault();
        openSessionsAndFocusFilter();
        return;
      }
      if (e.key === 'Escape' && sessionsOpen && !palette) {
        var tag = (e.target && e.target.tagName) ? e.target.tagName.toLowerCase() : '';
        if (e.target === ui.sessionsFilter || (tag !== 'input' && tag !== 'textarea' && tag !== 'select')) {
          setSessionsOpen(false, true);
        }
      }
    });
  }

  // ---- Inline activity rows (SPEC-dashboard-fullwidth FR-1217..FR-1221) ----
  // Grok-app style: the agent's processing shows in the chat stream as muted,
  // one-line, collapsible rows. They are rebuilt from the current snapshot on
  // every render and live in <li class="chat-aux"> elements between messages.

  var STEP_ICONS = { completed: '\u2713', running: '\u25CF', failed: '\u2717', skipped: '\u2013',
    waiting_gate: '\u23F8', ready: '\u25CB', pending: '\u25CB' };

  function fmtElapsed(ms) {
    var total = Math.max(0, Math.floor(ms / 1000));
    var h = Math.floor(total / 3600);
    var m = Math.floor((total % 3600) / 60);
    var sec = total % 60;
    if (h > 0) { return h + 'h' + (m ? ' ' + m + 'm' : ''); }
    if (m > 0) { return m + 'm' + (sec ? ' ' + sec + 's' : ''); }
    return sec + 's';
  }

  function serverNow() {
    return typeof window.dmuxaiServerNow === 'function' ? window.dmuxaiServerNow() : Date.now();
  }

  // A collapsible row: a one-line header button and an optional detail list.
  function auxRow(key, cls, headParts, detail) {
    var li = el('li', 'chat-aux ' + cls);
    li.setAttribute('data-aux-key', key);
    var head = el(detail ? 'button' : 'div', 'chat-aux-head');
    if (detail) {
      head.type = 'button';
      head.setAttribute('data-aux-key', key);
      var open = !!auxExpanded[key];
      head.setAttribute('aria-expanded', open ? 'true' : 'false');
      head.appendChild(el('span', 'chat-aux-caret'));
      head.lastChild.setAttribute('aria-hidden', 'true');
      head.addEventListener('click', function () {
        auxExpanded[key] = !auxExpanded[key];
        head.setAttribute('aria-expanded', auxExpanded[key] ? 'true' : 'false');
        detail.hidden = !auxExpanded[key];
      });
      detail.hidden = !open;
    }
    headParts.forEach(function (part) { if (part) { head.appendChild(part); } });
    li.appendChild(head);
    if (detail) { li.appendChild(detail); }
    return li;
  }

  // Live rows keep their elapsed time current between snapshots.
  function liveText(lineFn, live) {
    var span = el('span', 'chat-aux-text', lineFn());
    if (live) { span.dmuxLine = lineFn; }
    return span;
  }

  function tickLiveRows() {
    if (!ui.transcript) { return; }
    ui.transcript.querySelectorAll('li.chat-aux-live .chat-aux-text').forEach(function (span) {
      if (typeof span.dmuxLine === 'function') { span.textContent = span.dmuxLine(); }
    });
  }

  function graphElapsed(graph, now) {
    var start = null;
    var end = 0;
    (graph.nodes || []).forEach(function (n) {
      if (typeof n.started_at_ms === 'number' && (start === null || n.started_at_ms < start)) { start = n.started_at_ms; }
      if (typeof n.finished_at_ms === 'number' && n.finished_at_ms > end) { end = n.finished_at_ms; }
    });
    if (start === null) { return null; }
    var finished = graph.phase === 'completed' || graph.phase === 'failed';
    return (finished && end ? end : now) - start;
  }

  // FR-1217, FR-1218: the graph run as one status row with an expandable step
  // list; a step opens the Graph view focused on that node.
  function graphRunRow(graph) {
    var nodes = Array.isArray(graph.nodes) ? graph.nodes : [];
    if (!nodes.length || graph.phase === 'awaiting_approval') { return null; }
    var done = 0;
    var running = [];
    nodes.forEach(function (n) {
      var st = String(n.status || 'pending');
      if (st === 'completed' || st === 'skipped') { done++; }
      if (st === 'running') { running.push(String(n.id) + (n.runner ? ' on ' + n.runner : '')); }
    });
    var active = LIVE_PHASES.indexOf(String(graph.phase)) >= 0;
    var verb = active ? (running.length ? 'Running ' + running.join(', ') : 'Waiting') : finishedVerb(graph.phase);
    var parts = [active ? el('span', 'spinner') : phaseIcon(graph.phase)];
    var lineFn = function () {
      var line = verb + ' \u00b7 ' + done + '/' + nodes.length + ' steps';
      var ms = graphElapsed(graph, serverNow());
      if (ms !== null) { line += ' \u00b7 ' + fmtElapsed(ms); }
      return line;
    };
    parts.push(liveText(lineFn, active));
    var list = el('ul', 'chat-aux-steps');
    nodes.forEach(function (n) {
      var st = String(n.status || 'pending');
      var item = el('li', 'chat-aux-step chat-aux-step-' + st);
      var b = el('button', 'chat-aux-step-btn');
      b.type = 'button';
      b.title = 'Show ' + n.id + ' in the Graph view';
      b.appendChild(el('span', 'chat-aux-icon', STEP_ICONS[st] || '\u25CB'));
      b.lastChild.setAttribute('aria-hidden', 'true');
      b.appendChild(el('span', 'chat-aux-step-id', String(n.id)));
      b.appendChild(el('span', 'chat-aux-step-meta',
        [n.role, n.runner].filter(Boolean).join(' \u00b7 ') + ' \u00b7 ' + st.replace('_', ' ') +
        (typeof n.elapsed_ms === 'number' ? ' \u00b7 ' + fmtElapsed(n.elapsed_ms) : '')));
      b.addEventListener('click', function () {
        if (typeof window.dmuxaiFocusGraphNode === 'function') { window.dmuxaiFocusGraphNode(String(n.id)); }
      });
      item.appendChild(b);
      list.appendChild(item);
    });
    return withOpenButton(auxRow('graph:' + graph.id, 'chat-aux-run' + (active ? ' chat-aux-live' : ''), parts, list),
      graph.id, null);
  }

  // SPEC-dashboard-fullwidth FR-1223: phases where the run is still moving.
  var LIVE_PHASES = ['active', 'waiting_gate', 'triage', 'planning'];

  function finishedVerb(phase) {
    switch (String(phase)) {
      case 'failed': return 'Run failed';
      case 'cancelled': return 'Run stopped';
      case 'rejected': return 'Plan rejected';
      case 'awaiting_approval': return 'Plan waiting for approval';
      default: return 'Ran';
    }
  }

  function phaseIcon(phase) {
    var p = String(phase);
    var bad = p === 'failed';
    var neutral = p === 'cancelled' || p === 'rejected' || p === 'awaiting_approval';
    var icon = el('span', 'chat-aux-icon chat-aux-' + (bad ? 'failed' : neutral ? 'neutral' : 'ok'),
      bad ? '\u2717' : neutral ? '\u2013' : '\u2713');
    icon.setAttribute('aria-hidden', 'true');
    return icon;
  }

  // FR-1224: open the Graph view on a run of this conversation (an older run
  // is fetched by `show_graph_run`), optionally focused on one step.
  function showRun(runId, nodeId) {
    if (typeof window.dmuxaiShowGraphRun !== 'function') { return; }
    window.dmuxaiShowGraphRun(runId, nodeId).then(function (res) {
      if (res && res.error) { notify(res.error, true); }
    });
  }

  function withOpenButton(li, runId, band) {
    var b = el('button', 'chat-aux-open-run', 'Open in Graph');
    b.type = 'button';
    b.setAttribute('data-run-id', runId);
    var older = !!band;
    if (older && viewOnly) {
      b.disabled = true;
      b.title = 'The read-only view (dmuxai serve) shows the latest run only';
    } else {
      b.title = 'Show this run in the Graph view';
    }
    b.addEventListener('click', function () { showRun(runId, null); });
    var head = li.querySelector(':scope > .chat-aux-head');
    if (head && head.nextSibling) { li.insertBefore(b, head.nextSibling); } else { li.appendChild(b); }
    return li;
  }

  // FR-1223: a collapsed run row for an earlier run of this conversation,
  // built from its turn band (`graph.turns[]`).
  function bandRunRow(band) {
    var steps = Array.isArray(band.steps) ? band.steps : [];
    if (!steps.length) { return null; }
    var done = 0;
    var start = null;
    var end = null;
    var allEnded = true;
    steps.forEach(function (st) {
      var status = String(st.status || 'pending');
      if (status === 'completed' || status === 'skipped') { done++; }
      if (typeof st.started_at_ms === 'number' && (start === null || st.started_at_ms < start)) { start = st.started_at_ms; }
      if (typeof st.finished_at_ms === 'number') { end = end === null ? st.finished_at_ms : Math.max(end, st.finished_at_ms); }
      else { allEnded = false; }
    });
    var line = finishedVerb(LIVE_PHASES.indexOf(String(band.phase)) >= 0 ? 'cancelled' : band.phase) +
      ' \u00b7 ' + done + '/' + steps.length + ' steps';
    if (start !== null && end !== null && allEnded) { line += ' \u00b7 ' + fmtElapsed(end - start); }
    var parts = [phaseIcon(LIVE_PHASES.indexOf(String(band.phase)) >= 0 ? 'cancelled' : band.phase),
      el('span', 'chat-aux-text', line)];
    var list = el('ul', 'chat-aux-steps');
    steps.forEach(function (st) {
      var status = String(st.status || 'pending');
      var item = el('li', 'chat-aux-step chat-aux-step-' + status);
      var b = el('button', 'chat-aux-step-btn');
      b.type = 'button';
      b.title = 'Show ' + st.id + ' of this run in the Graph view';
      if (viewOnly) {
        b.disabled = true;
        b.title = 'The read-only view (dmuxai serve) shows the latest run only';
      }
      b.appendChild(el('span', 'chat-aux-icon', STEP_ICONS[status] || '\u25CB'));
      b.lastChild.setAttribute('aria-hidden', 'true');
      b.appendChild(el('span', 'chat-aux-step-id', String(st.id)));
      var ms = typeof st.started_at_ms === 'number' && typeof st.finished_at_ms === 'number'
        ? st.finished_at_ms - st.started_at_ms : null;
      b.appendChild(el('span', 'chat-aux-step-meta',
        [st.role, st.runner].filter(Boolean).join(' \u00b7 ') + ' \u00b7 ' + status.replace('_', ' ') +
        (ms !== null ? ' \u00b7 ' + fmtElapsed(ms) : '')));
      b.addEventListener('click', function () { showRun(String(band.run_id), String(st.id)); });
      item.appendChild(b);
      list.appendChild(item);
    });
    var li = auxRow('graph:' + band.run_id, 'chat-aux-run chat-aux-run-earlier', parts, list);
    li.setAttribute('data-run-id', String(band.run_id));
    return withOpenButton(li, String(band.run_id), band);
  }

  // Single-agent turn in progress: the live activity line from the snapshot.
  function agentRunRow(agent) {
    if (!agent || !agent.run || agent.run.state !== 'running') { return null; }
    var parts = [el('span', 'spinner')];
    var lineFn = function () {
      var line = String(agent.label || agent.id || 'agent');
      var act = agent.activity ? String(agent.activity) : 'working';
      line += ' \u00b7 ' + act.replace(/^\u25B8\s*/, '');
      if (typeof agent.run.started_at_ms === 'number') {
        line += ' \u00b7 ' + fmtElapsed(serverNow() - agent.run.started_at_ms);
      }
      return line;
    };
    parts.push(liveText(lineFn, true));
    return auxRow('agent:' + agent.id, 'chat-aux-run chat-aux-live', parts, null);
  }

  // FR-1221: a pending plan is an inline card with Approve / Edit / Reject.
  function approvalRow(graph) {
    if (!graph || graph.phase !== 'awaiting_approval' || !graph.approval) { return null; }
    var gate = window.dmuxaiChatPlanGate || null;
    var approval = graph.approval;
    var steps = Array.isArray(approval.steps) ? approval.steps.length : 0;
    var li = el('li', 'chat-aux chat-aux-approval');
    li.setAttribute('data-aux-key', 'approval:' + graph.id + ':' + approval.rev);
    li.setAttribute('role', 'group');
    li.setAttribute('aria-label', 'Plan approval');
    var head = el('div', 'chat-aux-head');
    head.appendChild(el('span', 'chat-aux-icon chat-aux-warn', '!'));
    head.lastChild.setAttribute('aria-hidden', 'true');
    head.appendChild(el('span', 'chat-aux-text',
      'Plan rev ' + approval.rev + ' is waiting for your approval \u00b7 ' + steps + (steps === 1 ? ' step' : ' steps')));
    li.appendChild(head);
    var actions = el('div', 'chat-aux-actions');
    var disabled = !gate || gate.disabled();
    [['Approve', 'approve', 'btn-primary'], ['Edit', 'edit', ''], ['Reject', 'reject', 'btn-danger'],
      ['Open graph', 'open', '']].forEach(function (spec) {
      var b = el('button', 'btn btn-small chat-aux-approval-' + spec[1] + (spec[2] ? ' ' + spec[2] : ''), spec[0]);
      b.type = 'button';
      b.disabled = spec[1] !== 'open' && disabled;
      b.addEventListener('click', function () { if (gate) { gate.action(spec[1]); } });
      actions.appendChild(b);
    });
    li.appendChild(actions);
    if (disabled && viewOnly) {
      li.appendChild(el('div', 'chat-aux-note', 'View-only: start dmuxai --web to decide from the browser.'));
    }
    return li;
  }

  // FR-1219: consecutive activity messages (tool calls, commands, thinking)
  // fold into one row: "N actions · <latest>"; expanding shows each line.
  function groupActivities() {
    var t = ui.transcript;
    var child = t.firstChild;
    while (child) {
      if (child.classList && child.classList.contains('msg-activity')) {
        var members = [];
        var cur = child;
        while (cur && cur.classList && cur.classList.contains('msg-activity')) {
          members.push(cur);
          cur = cur.nextSibling;
        }
        var key = 'acts:' + members[0].getAttribute('data-msg-id');
        var last = members[members.length - 1].dmuxText ? members[members.length - 1].dmuxText.textContent : '';
        var running = members.some(function (m) {
          var txt = m.dmuxText ? m.dmuxText.textContent : '';
          return txt.indexOf('\u2713') < 0 && txt.indexOf('\u2717') < 0 && /^\u25B8/.test(txt);
        });
        var parts = [];
        parts.push(running ? el('span', 'spinner') : el('span', 'chat-aux-icon', '\u2699'));
        if (!running) { parts[0].setAttribute('aria-hidden', 'true'); }
        parts.push(el('span', 'chat-aux-text',
          members.length + (members.length === 1 ? ' action' : ' actions') + ' \u00b7 ' + last.replace(/^\u25B8\s*/, '')));
        var detail = el('div', 'chat-aux-detail-marker');
        var row = auxRow(key, 'chat-aux-acts', parts, detail);
        // The members stay in place; the row only shows or hides them.
        detail.hidden = true;
        row.removeChild(detail);
        var open = !!auxExpanded[key];
        members.forEach(function (m) { m.hidden = !open; m.classList.add('msg-activity-grouped'); });
        var head = row.querySelector('.chat-aux-head');
        head.addEventListener('click', function () {
          members.forEach(function (m) { m.hidden = !auxExpanded[key]; });
        });
        t.insertBefore(row, members[0]);
        child = cur;
      } else {
        child = child.nextSibling;
      }
    }
  }

  // Index of the chat <li> after which the graph rows go: the last message of
  // the exchange whose user prompt started this graph, else the end.
  function graphAnchor(graph) {
    var t = ui.transcript;
    var prompt = String(graph.user_prompt || '').trim();
    var users = t.querySelectorAll(':scope > li.msg-user');
    var start = null;
    for (var i = users.length - 1; i >= 0 && prompt; i--) {
      if ((users[i].dmuxText ? users[i].dmuxText.textContent : '').trim() === prompt) { start = users[i]; break; }
    }
    return start ? exchangeEnd(start) : null;
  }

  // The last chat message of the exchange that starts at user message `start`.
  function exchangeEnd(start) {
    var anchor = start;
    var next = start.nextSibling;
    while (next && !(next.classList && next.classList.contains('msg-user'))) {
      if (next.classList && next.classList.contains('msg')) { anchor = next; }
      next = next.nextSibling;
    }
    return anchor;
  }

  function insertAfter(node, anchor) {
    var t = ui.transcript;
    if (anchor && anchor.parentNode === t) { t.insertBefore(node, anchor.nextSibling); }
    else { t.appendChild(node); }
  }

  function renderChatAux() {
    if (!ui.transcript) { return; }
    var t = ui.transcript;
    var active = document.activeElement;
    var focusKey = active && active.closest && t.contains(active) && active.getAttribute('data-aux-key');
    Array.prototype.slice.call(t.querySelectorAll(':scope > li.chat-aux')).forEach(function (n) { t.removeChild(n); });
    Array.prototype.slice.call(t.querySelectorAll(':scope > li.msg-activity-grouped')).forEach(function (n) {
      n.hidden = false;
      n.classList.remove('msg-activity-grouped');
    });
    groupActivities();
    var snap = lastSnap || {};
    var graph = snap.graph || null;
    if (graph) {
      // FR-1223: one row per exchange that had a run, from the turn bands.
      var placed = false;
      (Array.isArray(graph.turns) ? graph.turns : []).forEach(function (band) {
        if (!band || !band.message_id) { return; }
        var user = null;
        t.querySelectorAll(':scope > li.msg-user').forEach(function (li) {
          if (li.getAttribute('data-msg-id') === String(band.message_id)) { user = li; }
        });
        if (!user) { return; }
        var end = exchangeEnd(user);
        if (band.run_id === graph.id) {
          placed = true;
          var apprHere = approvalRow(graph);
          if (apprHere) { insertAfter(apprHere, end); }
          var cur = graphRunRow(graph);
          if (cur) { cur.setAttribute('data-run-id', String(graph.id)); insertAfter(cur, end); }
        } else {
          var row = bandRunRow(band);
          if (row) { insertAfter(row, end); }
        }
      });
      if (!placed) {
        var anchor = graphAnchor(graph);
        var runRow = graphRunRow(graph);
        var appr = approvalRow(graph);
        if (appr) { insertAfter(appr, anchor); }
        if (runRow) { runRow.setAttribute('data-run-id', String(graph.id)); insertAfter(runRow, anchor); }
      }
    }
    var graphLive = graph && graph.phase !== 'completed' && graph.phase !== 'failed';
    if (graphLive && t.querySelector(':scope > li.chat-aux-run.chat-aux-live')) {
      var busy = t.querySelector(':scope > li.msg-working');
      if (busy) { busy.parentNode.removeChild(busy); }
    }
    if (!graphLive) {
      (snap.agents || []).forEach(function (a) {
        var row = agentRunRow(a);
        if (row) {
          var working = t.querySelector(':scope > li.msg-working');
          if (working) { working.parentNode.removeChild(working); }
          t.appendChild(row);
        }
      });
    }
    if (focusKey) {
      var again = t.querySelector('button[data-aux-key="' + String(focusKey).replace(/["\\]/g, '\\$&') + '"]');
      if (again) { again.focus(); }
    }
  }

  // ---- ChatGPT-style layout (SPEC-dashboard-fullwidth FR-1205..FR-1208) ----

  // The chat panel fills the viewport below the sticky header; the header
  // wraps on narrow windows, so its height is measured, not assumed.
  function trackHeaderHeight() {
    var header = document.querySelector('header.top');
    if (!header) { return; }
    var sync = function () {
      document.documentElement.style.setProperty('--top-h', header.offsetHeight + 'px');
    };
    sync();
    if (typeof window.ResizeObserver === 'function') {
      new window.ResizeObserver(sync).observe(header);
    } else {
      window.addEventListener('resize', sync);
    }
  }

  // The composer grows with its text up to the CSS max-height, then scrolls.
  function autoGrowInput() {
    var input = ui.input;
    if (!input) { return; }
    input.style.height = 'auto';
    input.style.height = input.scrollHeight + 'px';
  }

  function scrollTranscriptToEnd() {
    if (ui.transcript) { ui.transcript.scrollTop = ui.transcript.scrollHeight; }
  }

  function hubAgent(id) {
    if (!lastSnap || !lastSnap.agents) { return null; }
    for (var i = 0; i < lastSnap.agents.length; i++) {
      if (lastSnap.agents[i].id === id) { return lastSnap.agents[i]; }
    }
    return null;
  }

  function runStatusLabel(agent, hub) {
    var run = (hub && hub.run) || (agent && agent.run);
    if (!run) {
      if (agent && agent.busy) { return 'busy'; }
      return '';
    }
    if (run.state === 'running') {
      var ms = typeof run.started_at_ms === 'number'
        ? Math.max(0, serverNow() - run.started_at_ms)
        : 0;
      return fmtElapsed(ms) || 'running';
    }
    if (run.state === 'done') {
      var dur = typeof run.duration_ms === 'number' ? fmtElapsed(run.duration_ms) : '';
      return dur ? ('✓ ' + dur) : '✓';
    }
    if (run.state === 'failed') {
      return '✗ failed';
    }
    if (run.state === 'cancelled') { return 'cancelled'; }
    return run.state || '';
  }

  function updateAgentPickerLabel() {
    if (!ui.agentPicker) { return; }
    var agent = selectedAgent();
    if (!agent) {
      ui.agentPicker.textContent = 'Agent';
      return;
    }
    var label = agent.label || agent.id;
    var model = agent.model ? (' · ' + agent.model) : '';
    ui.agentPicker.textContent = label + model;
    var color = agentColor(agent.id);
    if (color) {
      ui.agentPicker.style.setProperty('--agent-picker-dot', color);
    }
  }

  function closeAgentPopover() {
    if (agentPopover && agentPopover.parentNode) {
      agentPopover.parentNode.removeChild(agentPopover);
    }
    agentPopover = null;
    if (ui.agentPicker) { ui.agentPicker.setAttribute('aria-expanded', 'false'); }
  }

  function openAgentPopover() {
    if (!session || viewOnly) { return; }
    closeAgentPopover();
    var overlay = el('div', 'session-agent-overlay');
    var panel = el('div', 'session-agent-popover');
    panel.setAttribute('role', 'dialog');
    panel.setAttribute('aria-label', 'Choose agent and model');
    panel.tabIndex = -1;
    panel.appendChild(el('p', 'session-agent-popover-title', 'Agent for next message'));

    var list = el('ul', 'session-agent-list');
    session.agents.forEach(function (a) {
      var hub = hubAgent(a.id);
      var li = el('li', 'session-agent-item');
      var row = el('button', 'session-agent-row');
      row.type = 'button';
      if (a.id === session.selected_agent) { row.classList.add('session-agent-row-active'); }
      if (!a.allowed) { row.disabled = true; }
      var dot = el('span', 'session-agent-dot');
      var color = (hub && hub.color) || agentColor(a.id);
      if (color) { dot.style.backgroundColor = color; }
      row.appendChild(dot);
      var main = el('span', 'session-agent-main');
      main.appendChild(el('span', 'session-agent-name', a.label || a.id));
      var meta = el('span', 'session-agent-meta', (a.model || 'default model') +
        (runStatusLabel(a, hub) ? (' · ' + runStatusLabel(a, hub)) : ''));
      main.appendChild(meta);
      row.appendChild(main);
      row.addEventListener('click', function () {
        if (!a.allowed) { return; }
        command({ command: 'select_agent', agent: a.id }).then(function () {
          if (!a.models.length && !a.models_loading) {
            command({ command: 'load_models', agent: a.id });
          }
        });
      });
      li.appendChild(row);

      if (a.id === session.selected_agent) {
        var models = el('div', 'session-agent-models');
        if (a.models.length) {
          a.models.forEach(function (m) {
            var mb = el('button', 'session-model-chip' + (a.model === m.id ? ' on' : ''), m.label || m.id);
            mb.type = 'button';
            mb.addEventListener('click', function () {
              command({ command: 'select_model', agent: a.id, model: m.id });
            });
            models.appendChild(mb);
          });
        } else {
          var load = el('button', 'btn btn-small', a.models_loading ? 'Loading…' : 'Load models');
          load.type = 'button';
          load.disabled = !!a.models_loading;
          load.addEventListener('click', function () {
            command({ command: 'load_models', agent: a.id });
          });
          models.appendChild(load);
        }
        if (a.model) {
          var efforts = [];
          a.models.forEach(function (m) { if (m.id === a.model) { efforts = m.efforts || []; } });
          if (efforts.length) {
            var effortRow = el('div', 'session-agent-efforts');
            efforts.forEach(function (eff) {
              var eb = el('button', 'session-model-chip' + (a.effort === eff ? ' on' : ''), eff);
              eb.type = 'button';
              eb.addEventListener('click', function () {
                command({ command: 'set_effort', agent: a.id, effort: eff });
              });
              effortRow.appendChild(eb);
            });
            models.appendChild(effortRow);
          }
        }
        li.appendChild(models);
      }
      list.appendChild(li);
    });
    panel.appendChild(list);

    overlay.addEventListener('click', function (e) {
      if (e.target === overlay) { closeAgentPopover(); }
    });
    overlay.appendChild(panel);
    document.body.appendChild(overlay);
    agentPopover = overlay;
    ui.agentPicker.setAttribute('aria-expanded', 'true');
    panel.focus();
    document.addEventListener('keydown', agentPopoverKey, true);
  }

  function agentPopoverKey(e) {
    if (e.key !== 'Escape' || !agentPopover) { return; }
    e.preventDefault();
    e.stopPropagation();
    document.removeEventListener('keydown', agentPopoverKey, true);
    closeAgentPopover();
    if (ui.agentPicker) { ui.agentPicker.focus(); }
  }

  // FR-1207: fenced ``` blocks become <pre><code>, `inline` spans become
  // <code>; everything is a text node (no HTML parsing), so agent output can
  // not inject markup. An unclosed fence (still streaming) renders as code.
  function renderMessagePlain(target, content) {
    if (target.dmuxContent === content && !target.dmuxHtmlKey) { return; }
    target.dmuxContent = content;
    target.dmuxHtmlKey = '';
    target.classList.remove('md-body', 'msg-md');
    target.textContent = '';
    var text = String(content || '');
    var fence = /```([^\n`]*)\n?([\s\S]*?)(?:```|$)/g;
    var last = 0;
    var match;
    while ((match = fence.exec(text)) !== null) {
      // The line breaks around a fence belong to the block, not the prose.
      if (match.index > last) { appendInline(target, text.slice(last, match.index).replace(/\n+$/, '')); }
      var pre = el('pre', 'msg-code');
      var lang = match[1].trim();
      if (lang) { pre.setAttribute('data-lang', lang); }
      pre.appendChild(el('code', null, match[2].replace(/\n$/, '')));
      target.appendChild(pre);
      last = fence.lastIndex;
      if (match[0].length === 0) { fence.lastIndex++; }
      if (text.charAt(last) === '\n') { last++; }
    }
    if (last < text.length) { appendInline(target, text.slice(last).replace(/^\n+/, '')); }
  }

  function renderMessageText(target, message) {
    var content = String(message.content || '');
    var htmlKey = message.content_html || '';
    if (message.role === 'agent' && htmlKey) {
      if (target.dmuxContent === content && target.dmuxHtmlKey === htmlKey) { return; }
      target.dmuxContent = content;
      target.dmuxHtmlKey = htmlKey;
      target.classList.add('md-body', 'msg-md');
      target.innerHTML = htmlKey;
      return;
    }
    if (message.role === 'user') {
      if (target.dmuxContent === content && !target.dmuxHtmlKey) { return; }
      target.dmuxContent = content;
      target.dmuxHtmlKey = '';
      target.classList.remove('md-body', 'msg-md');
      target.textContent = '';
      target.appendChild(document.createTextNode(content));
      return;
    }
    renderMessagePlain(target, content);
  }

  function appendInline(target, text) {
    var parts = text.split(/`([^`\n]+)`/);
    for (var i = 0; i < parts.length; i++) {
      if (!parts[i]) { continue; }
      if (i % 2 === 1) {
        target.appendChild(el('code', 'msg-inline-code', parts[i]));
      } else {
        target.appendChild(document.createTextNode(parts[i]));
      }
    }
  }

  function picker(label, parity, onChange) {
    var wrap = el('label', 'session-picker');
    wrap.appendChild(el('span', 'field-label', label));
    var select = el('select', 'session-select');
    select.setAttribute('data-parity', parity);
    select.addEventListener('change', onChange);
    wrap.appendChild(select);
    return select;
  }

  function setOptions(select, options, value) {
    var key = JSON.stringify(options) + '|' + value;
    if (select.getAttribute('data-key') === key) { return; }
    select.setAttribute('data-key', key);
    select.textContent = '';
    options.forEach(function (opt) {
      var o = el('option', null, opt.label);
      o.value = opt.value;
      if (opt.disabled) { o.disabled = true; }
      select.appendChild(o);
    });
    select.value = value;
  }

  function onPromptKey(e) {
    if (e.isComposing || e.keyCode === 229) { return; }
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      submitPrompt();
    } else if (e.key === 'Escape') {
      closePalette();
    }
  }

  function submitPrompt() {
    var text = ui.input.value;
    if (!text.trim()) { return; }
    ui.send.disabled = true;
    sendPrompt(text).then(function (ok) {
      ui.send.disabled = viewOnly;
      if (ok) {
        ui.input.value = '';
        autoGrowInput();
        // UX-1203: your own message always brings the view to the latest turn.
        stickToBottom = true;
        scrollTranscriptToEnd();
      }
      ui.input.focus();
    });
  }

  // ---- render -----------------------------------------------------------

  function render(next) {
    if (!next) { return; }
    session = next;
    var agent = selectedAgent();
    var busy = !!(agent && agent.busy);

    ui.title.textContent = session.conversation_title
      ? session.conversation_title + ' · ' + session.conversation_id.slice(0, 8)
      : session.conversation_id.slice(0, 8);

    updateAgentPickerLabel();
    if (ui.agentPicker) { ui.agentPicker.disabled = viewOnly; }
    if (agentPopover) { closeAgentPopover(); }
    ui.cancel.hidden = !busy;
    ui.details.classList.toggle('btn-on', !!session.details_expanded);
    [ui.newConv, ui.status, ui.details, ui.share, ui.palette, ui.quit, ui.send].forEach(function (b) {
      b.disabled = viewOnly;
    });
    if (ui.route) { ui.route.disabled = viewOnly; }
    ui.input.disabled = viewOnly;
    renderTranscript();
    renderConversations();
  }

  function renderTranscript() {
    if (conversationShown !== session.conversation_id) {
      conversationShown = session.conversation_id;
      ui.transcript.textContent = '';
      messageNodes = {};
      stickToBottom = true;
    }
    var seen = {};
    var previous = null;
    session.transcript.forEach(function (m) {
      seen[m.id] = true;
      var node = messageNodes[m.id];
      if (!node) {
        node = el('li', 'msg msg-' + m.role);
        node.setAttribute('data-msg-id', m.id);
        node.dmuxWho = el('span', 'msg-who');
        node.dmuxText = el('div', 'msg-text');
        node.appendChild(node.dmuxWho);
        node.appendChild(node.dmuxText);
        messageNodes[m.id] = node;
        if (previous && previous.nextSibling) {
          ui.transcript.insertBefore(node, previous.nextSibling);
        } else if (!previous) {
          ui.transcript.insertBefore(node, ui.transcript.firstChild);
        } else {
          ui.transcript.appendChild(node);
        }
      }
      var who = m.role === 'agent' ? (m.agent || 'agent') : m.role;
      if (node.dmuxWho.textContent !== who) { node.dmuxWho.textContent = who; }
      // The agent color marks the message edge; the label keeps the theme's
      // text color so it stays readable for any configured agent color.
      var color = m.agent ? agentColor(m.agent) : null;
      if (color) { node.style.setProperty('--agent-color', color); } else { node.style.removeProperty('--agent-color'); }
      renderMessageText(node.dmuxText, m);
      node.classList.toggle('msg-streaming', !!m.streaming);
      previous = node;
    });
    Object.keys(messageNodes).forEach(function (id) {
      if (!seen[id]) {
        var gone = messageNodes[id];
        if (gone.parentNode) { gone.parentNode.removeChild(gone); }
        delete messageNodes[id];
      }
    });
    if (!session.transcript.length) {
      if (!ui.transcript.querySelector('.session-empty')) {
        ui.transcript.appendChild(el('li', 'session-empty', 'No messages yet.'));
      }
    } else {
      var empty = ui.transcript.querySelector('.session-empty');
      if (empty) { empty.parentNode.removeChild(empty); }
    }
    var currentAgent = selectedAgent();
    var activeRun = currentAgent && currentAgent.run && currentAgent.run.state === 'running';
    var hasStreaming = session.transcript.some(function (m) { return !!m.streaming; });
    var workingNode = ui.transcript.querySelector('.msg-working');
    if (activeRun && !hasStreaming) {
      if (!workingNode) {
        workingNode = el('li', 'msg msg-agent msg-working');
        workingNode.appendChild(el('span', 'msg-who', currentAgent.label || currentAgent.id));
        var textNode = el('div', 'msg-text');
        textNode.appendChild(el('span', 'spinner'));
        textNode.appendChild(document.createTextNode(' working\u2026'));
        workingNode.appendChild(textNode);
        ui.transcript.appendChild(workingNode);
      }
    } else if (workingNode) {
      workingNode.parentNode.removeChild(workingNode);
    }
    renderChatAux();
    // docs.js links agent turns that changed files to their chat message
    // (SPEC-dashboard-fullwidth FR-1209, FR-1210).
    window.dispatchEvent(new window.CustomEvent('dmuxai:transcript-rendered'));
    if (stickToBottom) { scrollTranscriptToEnd(); }
  }

  function openSessionsAndFocusFilter() {
    setSessionsOpen(true, true);
    if (ui.sessionsFilter) { ui.sessionsFilter.focus(); }
  }

  // SPEC-dashboard-fullwidth FR-1225..FR-1227: the Sessions list is grouped
  // by workspace folder (current workspace first), 3 rows per group with
  // More/Less, compact relative times, and per-tab collapse state.
  var SESSIONS_GROUP_VISIBLE = 3;
  var SESSIONS_COLLAPSED_KEY = 'dmuxai.sessions.collapsed';
  var sessionsMore = {};

  function sessionsCollapsed() {
    try {
      var raw = window.sessionStorage.getItem(SESSIONS_COLLAPSED_KEY);
      var parsed = raw ? JSON.parse(raw) : {};
      return parsed && typeof parsed === 'object' ? parsed : {};
    } catch (err) { return {}; }
  }

  function setSessionsCollapsed(path, collapsed) {
    var map = sessionsCollapsed();
    if (collapsed) { map[path] = true; } else { delete map[path]; }
    try { window.sessionStorage.setItem(SESSIONS_COLLAPSED_KEY, JSON.stringify(map)); } catch (err) { /* ignore */ }
  }

  function compactTime(ms) {
    var ts = Number(ms) || 0;
    if (!ts) { return ''; }
    var sec = Math.max(0, Math.floor((Date.now() - ts) / 1000));
    if (sec < 60) { return sec + 's'; }
    var min = Math.floor(sec / 60);
    if (min < 60) { return min + 'm'; }
    var hr = Math.floor(min / 60);
    if (hr < 24) { return hr + 'h'; }
    var day = Math.floor(hr / 24);
    if (day < 7) { return day + 'd'; }
    if (day < 365) { return Math.floor(day / 7) + 'w'; }
    return Math.floor(day / 365) + 'y';
  }

  function folderIcon() {
    var ns = 'http://www.w3.org/2000/svg';
    var svg = document.createElementNS(ns, 'svg');
    svg.setAttribute('class', 'sessions-group-icon');
    svg.setAttribute('viewBox', '0 0 16 16');
    svg.setAttribute('aria-hidden', 'true');
    var path = document.createElementNS(ns, 'path');
    path.setAttribute('d', 'M1.5 3.5h4.2l1.5 1.5h7.3v7.5a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1z');
    path.setAttribute('fill', 'none');
    path.setAttribute('stroke', 'currentColor');
    path.setAttribute('stroke-width', '1.3');
    path.setAttribute('stroke-linejoin', 'round');
    svg.appendChild(path);
    return svg;
  }

  function sessionGroups() {
    var groups = Array.isArray(session.workspaces) ? session.workspaces.slice() : [];
    var current = null;
    groups = groups.filter(function (g) {
      if (g && g.current) { current = g; return false; }
      return !!g;
    });
    if (!current) {
      current = { path: '', name: 'This workspace', label: '', current: true, available: true };
    }
    var currentGroup = {
      path: current.path, name: current.name, label: current.label,
      current: true, available: true, conversations: session.conversations || []
    };
    groups.sort(function (a, b) { return (Number(b.updated_at_ms) || 0) - (Number(a.updated_at_ms) || 0); });
    return [currentGroup].concat(groups);
  }

  function focusSessionRow(from, delta) {
    var rows = Array.prototype.slice.call(ui.sessionsList.querySelectorAll('.sessions-row'));
    var index = rows.indexOf(from);
    var next = rows[index + delta];
    if (next) { next.focus(); }
  }

  function openOtherWorkspaceConversation(group, c) {
    var switcher = window.dmuxaiSwitchWorkspace;
    if (typeof switcher !== 'function') {
      notify('Switching workspace is not available in this dashboard.', true);
      return;
    }
    command({ command: 'prepare_open_conversation', workspace: group.path, id: c.id }).then(function (data) {
      if (!data) { return; }
      notify('Switching to ' + (group.name || group.path) + '\u2026');
      switcher(group.path).then(function (res) {
        if (!res || !res.ok) { notify((res && res.error) || 'Workspace switch failed.', true); }
      });
    });
  }

  function sessionRow(group, c, canSwitch) {
    var isActive = group.current && c.id === session.conversation_id;
    var reason = '';
    if (viewOnly) {
      reason = 'View-only dashboard';
    } else if (!group.current && !group.available) {
      reason = 'This workspace folder no longer exists';
    } else if (!group.current && !canSwitch) {
      reason = 'Switching workspace is not available in this dashboard';
    }
    var disabled = !!reason;
    var li = el('li', 'sessions-row' + (isActive ? ' sessions-row-active' : '') + (disabled ? ' sessions-row-disabled' : ''));
    li.tabIndex = 0;
    li.setAttribute('data-id', c.id);
    li.setAttribute('data-parity', 'resume');
    li.setAttribute('data-agent', c.last_agent || '');
    if (disabled) { li.setAttribute('aria-disabled', 'true'); }
    if (isActive) { li.setAttribute('aria-current', 'true'); }

    var titleText = c.title || c.id.slice(0, 8);
    var tip = titleText + (c.last_agent ? ' \u00b7 ' + c.last_agent : '');
    if (!group.current && !disabled) { tip += ' \u00b7 opens in ' + (group.name || group.path); }
    if (reason) { tip += ' \u2014 ' + reason; }
    li.title = tip;

    var top = el('div', 'sessions-row-head');
    if (c.running) {
      var spin = el('span', 'spinner');
      spin.setAttribute('role', 'img');
      spin.setAttribute('aria-label', 'running');
      top.appendChild(spin);
    }
    top.appendChild(el('span', 'sessions-row-title', titleText));
    top.appendChild(el('span', 'sessions-row-agent sr-only', c.last_agent || 'none'));
    var timeEl = el('span', 'sessions-row-time', compactTime(c.updated_at_ms));
    if (c.updated_at_ms) {
      try { timeEl.title = new Date(c.updated_at_ms).toISOString(); } catch (err) { /* ignore */ }
    }
    top.appendChild(timeEl);
    li.appendChild(top);

    function activateRow() {
      if (disabled || isActive) { return; }
      if (group.current) {
        command({ command: 'load_conversation', id: c.id });
      } else {
        openOtherWorkspaceConversation(group, c);
      }
    }
    li.addEventListener('click', activateRow);
    li.addEventListener('keydown', function (e) {
      if (e.isComposing || e.keyCode === 229) { return; }
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        activateRow();
      } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        focusSessionRow(li, 1);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        focusSessionRow(li, -1);
      }
    });
    return li;
  }

  function renderConversations() {
    if (!ui.sessionsList) { return; }
    if (ui.sessionsNew) { ui.sessionsNew.disabled = viewOnly || !session; }
    if (ui.sessionsViewOnly) { ui.sessionsViewOnly.hidden = !viewOnly; }
    if (!session || !session.conversations) {
      ui.sessionsList.removeAttribute('data-key');
      ui.sessionsList.textContent = '';
      ui.sessionsList.appendChild(el('li', 'sessions-empty', 'No conversations yet.'));
      return;
    }
    var q = (sessionsFilter || '').trim().toLowerCase();
    var canSwitch = typeof window.dmuxaiWorkspaceCanSwitch === 'function' && window.dmuxaiWorkspaceCanSwitch();
    var collapsed = sessionsCollapsed();
    var key = JSON.stringify([session.conversations, session.workspaces || [], session.conversation_id, q,
      viewOnly, canSwitch, collapsed, sessionsMore, Math.floor(Date.now() / 60000)]);
    if (ui.sessionsList.getAttribute('data-key') === key) { return; }
    ui.sessionsList.setAttribute('data-key', key);
    ui.sessionsList.textContent = '';

    function matches(c) {
      if (!q) { return true; }
      var title = (c.title || '').toLowerCase();
      var id = (c.id || '').toLowerCase();
      var agent = (c.last_agent || '').toLowerCase();
      return title.indexOf(q) >= 0 || id.indexOf(q) >= 0 || agent.indexOf(q) >= 0;
    }

    var shown = 0;
    sessionGroups().forEach(function (group) {
      var list = (group.conversations || []).filter(matches).slice().sort(function (a, b) {
        return (Number(b.updated_at_ms) || 0) - (Number(a.updated_at_ms) || 0);
      });
      if (!list.length && (q || !group.current)) { return; }
      var path = group.path || '';
      var isCollapsed = !q && !!collapsed[path];
      var gli = el('li', 'sessions-group' + (group.current ? ' sessions-group-current' : '') + (isCollapsed ? ' sessions-group-collapsed' : ''));
      gli.setAttribute('data-workspace', path);
      var head = el('button', 'sessions-group-head');
      head.type = 'button';
      head.title = path;
      head.setAttribute('aria-expanded', isCollapsed ? 'false' : 'true');
      head.appendChild(folderIcon());
      head.appendChild(el('span', 'sessions-group-name', group.name || path || 'Workspace'));
      if (group.label) { head.appendChild(el('span', 'sessions-group-label', group.label)); }
      head.addEventListener('click', function () {
        setSessionsCollapsed(path, !isCollapsed);
        renderConversations();
      });
      gli.appendChild(head);

      var ul = el('ul', 'sessions-group-list');
      ul.hidden = isCollapsed;
      var expanded = !!q || !!sessionsMore[path];
      var visible = expanded ? list : list.slice(0, SESSIONS_GROUP_VISIBLE);
      if (!expanded && group.current) {
        var active = list.filter(function (c) { return c.id === session.conversation_id; })[0];
        if (active && visible.indexOf(active) < 0) { visible = visible.concat([active]); }
      }
      visible.forEach(function (c) { ul.appendChild(sessionRow(group, c, canSwitch)); });
      if (!list.length) {
        ul.appendChild(el('li', 'sessions-empty', 'No conversations yet.'));
      }
      if (!q && list.length > SESSIONS_GROUP_VISIBLE) {
        var moreLi = el('li', 'sessions-more');
        var more = el('button', 'sessions-more-btn', expanded ? 'Less' : 'More');
        more.type = 'button';
        more.setAttribute('aria-expanded', expanded ? 'true' : 'false');
        if (!expanded) { more.title = (list.length - visible.length) + ' more'; }
        more.addEventListener('click', function () {
          if (expanded) { delete sessionsMore[path]; } else { sessionsMore[path] = true; }
          renderConversations();
        });
        moreLi.appendChild(more);
        ul.appendChild(moreLi);
      }
      gli.appendChild(ul);
      ui.sessionsList.appendChild(gli);
      shown += list.length;
    });

    if (!shown && q) {
      ui.sessionsList.textContent = '';
      ui.sessionsList.appendChild(el('li', 'sessions-empty', 'No matching sessions.'));
    }
  }

  // ---- palette ----------------------------------------------------------

  var ACTIONS = {
    '/agent': function () { ask('Agent', agentChoices(), function (v) { command({ command: 'select_agent', agent: v }); }); },
    '/mode': function () { notify('Mode selection was removed; every turn runs in agent mode.', true); },
    '/effort': function () {
      var agent = selectedAgent();
      var efforts = [];
      if (agent && agent.model) {
        agent.models.forEach(function (m) { if (m.id === agent.model) { efforts = m.efforts; } });
      }
      if (!efforts.length) { notify('Reasoning effort is available for Codex models that offer it.', true); return; }
      ask('Effort', efforts.map(choice), function (v) { command({ command: 'set_effort', agent: session.selected_agent, effort: v }); });
    },
    '/status': function () { command({ command: 'status' }); },
    '/quit': function () { ui.quit.click(); },
    '/new': function () { command({ command: 'new_conversation' }); },
    '/conversations': function () { openSessionsAndFocusFilter(); },
    '/resume': function () {
      ask('Conversation', session.conversations.map(function (c) {
        return { value: c.id, label: (c.title || c.id.slice(0, 8)) + ' · ' + c.id.slice(0, 8) };
      }), function (v) { command({ command: 'load_conversation', id: v }); });
    },
    '/summarize': function () { askText('Focus (optional)', function (t) { sendPrompt(t ? '/summarize ' + t : '/summarize'); }); },
    '/compact': function () { askText('Focus (optional)', function (t) { sendPrompt(t ? '/compact ' + t : '/compact'); }); },
    '/plan': function () { askText('Prompt (plan first, then approve)', function (t) { if (t) { sendPrompt('/plan ' + t); } }, true); },
    '/direct': function () { askText('Prompt (runs without planning)', function (t) { if (t) { sendPrompt('/direct ' + t); } }, true); },
    '/share': function () { openShare(); },
    '/image': function () { askText('Image path in the workspace', function (t) { if (t) { sendPrompt('/image ' + t); } }, true); },
    '/details': function () { command({ command: 'toggle_details' }); },
    '/remember': function () {
      askText('Note (prefix --human, --decision <name> or --gotcha <name>)', function (t) {
        if (t) { sendPrompt('/remember ' + t); }
      }, true);
    },
    '/memory': function () {
      ask('Memory', [
        { value: '', label: 'List proposals' },
        { value: 'approve', label: 'Approve a proposal…' },
        { value: 'reject', label: 'Reject a proposal…' }
      ], function (v) {
        if (!v) { sendPrompt('/memory'); return; }
        askText('Proposal id', function (id) { if (id) { sendPrompt('/memory ' + v + ' ' + id); } }, true);
      });
    },
    '/graph': function () {
      ask('Graph', [choice('status'), choice('continue'), { value: 'override', label: 'override…' }], function (v) {
        if (v === 'override') {
          askText('Override text', function (t) { if (t) { sendPrompt('/graph override ' + t); } }, true);
        } else {
          sendPrompt('/graph ' + v);
        }
      });
    }
  };

  var PARITY_IDS = {
    '/agent': 'agent', '/mode': 'mode', '/effort': 'effort', '/status': 'status', '/quit': 'quit',
    '/new': 'new', '/conversations': 'conversations', '/resume': 'resume', '/summarize': 'summarize',
    '/compact': 'compact', '/plan': 'route', '/direct': 'route', '/share': 'share', '/image': 'image', '/details': 'details', '/graph': 'graph',
    '/remember': 'remember', '/memory': 'memory'
  };

  function choice(v) { return { value: v, label: v }; }

  function agentChoices() {
    return session.agents.map(function (a) {
      return { value: a.id, label: a.label + (a.allowed ? '' : ' (not allowed)'), disabled: !a.allowed };
    });
  }

  function openShare() {
    var targets = session.agents.filter(function (a) {
      return SHARE_TARGETS.indexOf(a.id) >= 0 && a.id !== session.selected_agent;
    }).map(function (a) {
      return { value: a.id, label: 'Share with ' + a.label, disabled: !a.allowed || a.busy };
    });
    ask('Share transcript', targets, function (v) { command({ command: 'share', target: v }); });
  }

  function openPalette(filter, fromSlash) {
    if (viewOnly || !session) { return; }
    var entries = session.commands.map(function (c) {
      return { value: c.name, label: c.name, hint: c.description, parity: PARITY_IDS[c.name] };
    });
    showOverlay('Commands', entries, function (v) {
      var action = ACTIONS[v];
      if (action) { action(); } else { sendPrompt(v); }
    }, filter, true, fromSlash ? function () {
      // Esc after typing "/" keeps the slash so a raw command can be typed.
      ui.input.value = '/';
    } : null);
  }

  function ask(title, entries, onPick) {
    showOverlay(title, entries, onPick, '', entries.length > 6);
  }

  function askText(title, onDone, required) {
    closePalette();
    var overlay = el('div', 'session-overlay');
    overlay.setAttribute('data-parity', 'escape');
    var box = el('div', 'session-dialog');
    box.appendChild(el('h3', 'session-subtitle', title));
    var input = el('input', 'session-filter');
    input.type = 'text';
    box.appendChild(input);
    var ok = button('OK', null, function () {
      var value = input.value.trim();
      if (required && !value) { input.focus(); return; }
      closePalette();
      onDone(value);
    }, 'btn-primary');
    box.appendChild(ok);
    input.addEventListener('keydown', function (e) {
      if (e.isComposing || e.keyCode === 229) { return; }
      if (e.key === 'Enter') { e.preventDefault(); ok.click(); }
      if (e.key === 'Escape') { e.preventDefault(); closePalette(); }
    });
    overlay.appendChild(box);
    overlay.addEventListener('click', function (e) { if (e.target === overlay) { closePalette(); } });
    document.body.appendChild(overlay);
    palette = overlay;
    input.focus();
  }

  function showOverlay(title, entries, onPick, filter, withFilter, onEscape) {
    closePalette();
    var overlay = el('div', 'session-overlay');
    overlay.setAttribute('data-parity', 'escape');
    overlay.setAttribute('role', 'dialog');
    overlay.setAttribute('aria-label', title);
    var box = el('div', 'session-dialog');
    box.appendChild(el('h3', 'session-subtitle', title));
    var input = el('input', 'session-filter');
    input.type = 'text';
    input.placeholder = 'Type to filter';
    input.value = filter || '';
    input.hidden = !withFilter;
    box.appendChild(input);
    var list = el('ul', 'session-options');
    box.appendChild(list);
    var cursor = 0;
    var visible = [];

    function draw() {
      var q = input.value.trim().toLowerCase();
      visible = entries.filter(function (e) {
        return !q || e.label.toLowerCase().indexOf(q) >= 0 || (e.hint || '').toLowerCase().indexOf(q) >= 0;
      });
      if (cursor >= visible.length) { cursor = Math.max(0, visible.length - 1); }
      list.textContent = '';
      visible.forEach(function (entry, i) {
        var li = el('li', 'session-option' + (i === cursor ? ' session-option-active' : '') + (entry.disabled ? ' session-option-disabled' : ''));
        if (entry.parity) { li.setAttribute('data-parity', entry.parity); }
        li.appendChild(el('span', 'session-option-label', entry.label));
        if (entry.hint) { li.appendChild(el('span', 'session-option-hint', entry.hint)); }
        li.addEventListener('click', function () { pick(entry); });
        list.appendChild(li);
      });
      if (!visible.length) { list.appendChild(el('li', 'session-empty', 'No matches')); }
    }

    function pick(entry) {
      if (!entry || entry.disabled) { return; }
      closePalette();
      onPick(entry.value);
    }

    input.addEventListener('input', function () { cursor = 0; draw(); });
    overlay.addEventListener('keydown', function (e) {
      if (e.isComposing || e.keyCode === 229) { return; }
      if (e.key === 'Escape') {
        e.preventDefault();
        closePalette();
        if (onEscape) { onEscape(); }
        ui.input.focus();
      }
      else if (e.key === 'ArrowDown') { e.preventDefault(); cursor = Math.min(cursor + 1, visible.length - 1); draw(); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); cursor = Math.max(cursor - 1, 0); draw(); }
      else if (e.key === 'Enter') { e.preventDefault(); pick(visible[cursor]); }
    });
    overlay.addEventListener('click', function (e) { if (e.target === overlay) { closePalette(); } });
    overlay.appendChild(box);
    document.body.appendChild(overlay);
    palette = overlay;
    draw();
    overlay.tabIndex = -1;
    if (withFilter) { input.focus(); } else { overlay.focus(); }
  }

  function closePalette() {
    if (palette && palette.parentNode) { palette.parentNode.removeChild(palette); }
    palette = null;
  }

  // ---- data -------------------------------------------------------------

  function applySnapshot(snap) {
    if (!snap) { return; }
    lastSnap = snap;
    if (snap.workspace) {
      initSessionsSidebarForWorkspace(snap.workspace);
    }
    var colors = {};
    (snap.agents || []).forEach(function (a) { if (a.color) { colors[a.id] = a.color; } });
    window.__dmuxSessionAgents = colors;
    viewOnly = snap.source === 'files';
    if (ui.sessionsNew) { ui.sessionsNew.disabled = viewOnly || !snap.session; }
    if (ui.sessionsViewOnly) { ui.sessionsViewOnly.hidden = !viewOnly; }
    // `--web` publishes the session with its first snapshot; wait for it.
    root.hidden = !viewOnly && !snap.session;
    ui.viewOnly.hidden = !viewOnly;
    if (snap.session) {
      render(snap.session);
    } else if (viewOnly) {
      root.classList.add('session-disabled');
      session = null;
      [ui.agentPicker, ui.input, ui.send].forEach(function (c) { if (c) { c.disabled = true; } });
      renderConversations();
      renderChatAux();
    }
  }

  // app.js owns the SSE stream and polling fallback; it re-dispatches every
  // snapshot. Snapshots that arrive before the DOM is ready are kept.
  document.addEventListener('dmuxai:snapshot', function (e) {
    if (!root) { pending = e.detail; return; }
    applySnapshot(e.detail);
  });

  document.addEventListener('DOMContentLoaded', function () {
    root = document.getElementById('session');
    if (!root) { return; }
    build();
    root.hidden = true;
    if (pending) { applySnapshot(pending); pending = null; }
  });

  // Plan-gate approval buttons are rendered in app.js via parityButton (parity ids).
  window.dmuxaiSession = { parityButton: button };
})();
