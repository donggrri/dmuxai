'use strict';
// SPEC-dashboard-parity FR-780..FR-784, UX-780: chat session, pickers,
// command palette and lifecycle controls for `dmuxai --web`. Self-contained:
// it shares only the token in sessionStorage, the <section id="session">
// container and app.js's `dmuxai:snapshot` DOM event (one SSE connection).

(function () {
  var TOKEN_KEY = 'dmuxai.token';
  var MODES = ['ask', 'plan', 'implement'];
  var SHARE_TARGETS = ['codex', 'cursor', 'agy'];

  function storageGet(key) {
    try { return window.sessionStorage.getItem(key); } catch (err) { return null; }
  }

  function storageSet(key, value) {
    try { window.sessionStorage.setItem(key, value); } catch (err) { /* ignore */ }
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
  var palette = null;

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) { node.className = className; }
    if (text !== undefined && text !== null) { node.textContent = text; }
    return node;
  }

  function button(label, parity, onClick, className) {
    var b = el('button', 'btn' + (className ? ' ' + className : ''), label);
    b.type = 'button';
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

  function build() {
    root.textContent = '';
    root.classList.add('session');

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
    ui.agent = picker('Agent', 'agent', function () {
      command({ command: 'select_agent', agent: ui.agent.value });
    });
    ui.mode = picker('Mode', 'mode', function () {
      command({ command: 'set_mode', agent: session.selected_agent, mode: ui.mode.value });
    });
    ui.model = picker('Model', 'model', function () {
      if (ui.model.value === '__load__') {
        command({ command: 'load_models', agent: session.selected_agent });
        return;
      }
      if (ui.model.value) {
        command({ command: 'select_model', agent: session.selected_agent, model: ui.model.value });
      }
    });
    ui.model.addEventListener('focus', function () {
      var agent = selectedAgent();
      if (agent && !agent.models.length && !agent.models_loading) {
        command({ command: 'load_models', agent: agent.id });
      }
    });
    ui.effort = picker('Effort', 'effort', function () {
      command({ command: 'set_effort', agent: session.selected_agent, effort: ui.effort.value });
    });
    [ui.agent, ui.mode, ui.model, ui.effort].forEach(function (select) {
      bar.appendChild(select.parentNode);
    });

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

    var side = el('aside', 'session-conversations');
    side.setAttribute('data-parity', 'conversations');
    side.appendChild(el('h3', 'session-subtitle', 'Conversations'));
    ui.conversations = el('ul', 'session-conv-list');
    side.appendChild(ui.conversations);
    body.appendChild(side);
    root.appendChild(body);

    ui.notice = el('p', 'session-notice');
    ui.notice.hidden = true;
    ui.notice.setAttribute('role', 'status');
    root.appendChild(ui.notice);

    var form = el('div', 'session-prompt');
    ui.input = el('textarea', 'session-input');
    ui.input.rows = 3;
    ui.input.placeholder = 'Message the selected agent. Enter sends, Shift+Enter adds a line, / opens commands.';
    ui.input.setAttribute('data-parity', 'prompt-newline');
    ui.input.setAttribute('aria-label', 'Prompt');
    ui.input.addEventListener('keydown', onPromptKey);
    ui.input.addEventListener('input', function () {
      if (ui.input.value === '/') {
        ui.input.value = '';
        openPalette('', true);
      }
    });
    form.appendChild(ui.input);
    ui.send = button('Send', 'prompt-send', submitPrompt, 'btn-primary');
    form.appendChild(ui.send);
    root.appendChild(form);

    document.addEventListener('keydown', function (e) {
      if ((e.ctrlKey || e.metaKey) && (e.key === 'k' || e.key === 'K')) {
        e.preventDefault();
        openPalette('');
      }
    });
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
      if (ok) { ui.input.value = ''; }
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

    setOptions(ui.agent, session.agents.map(function (a) {
      var stateSuffix = '';
      if (a.run && a.run.state === 'running') {
        stateSuffix = ' · ⏳ running';
      } else if (a.run && a.run.state === 'done') {
        stateSuffix = ' · ✓ done';
      } else if (a.run && a.run.state === 'failed') {
        stateSuffix = ' · ✗ failed';
      } else if (a.run && a.run.state === 'cancelled') {
        stateSuffix = ' · ■ cancelled';
      } else if (a.busy) {
        stateSuffix = ' · busy';
      }
      return {
        value: a.id,
        label: a.label + (a.allowed ? '' : ' (not allowed)') + stateSuffix,
        disabled: !a.allowed
      };
    }), session.selected_agent);

    var modes = (agent && agent.modes.length) ? agent.modes : MODES;
    setOptions(ui.mode, modes.map(function (m) {
      return { value: m, label: m.charAt(0).toUpperCase() + m.slice(1) };
    }), agent ? agent.mode : 'ask');

    var modelOptions = [];
    if (agent && agent.models.length) {
      modelOptions = agent.models.map(function (m) { return { value: m.id, label: m.label }; });
      if (!agent.model) { modelOptions.unshift({ value: '', label: 'provider default' }); }
    } else {
      modelOptions.push({ value: agent && agent.model ? agent.model : '', label: agent && agent.model ? agent.model : 'provider default' });
      modelOptions.push({ value: '__load__', label: agent && agent.models_loading ? 'Loading models…' : 'Load models…' });
    }
    setOptions(ui.model, modelOptions, agent && agent.model ? agent.model : '');

    var efforts = [];
    if (agent && agent.model) {
      agent.models.forEach(function (m) { if (m.id === agent.model) { efforts = m.efforts; } });
    }
    ui.effort.parentNode.hidden = !efforts.length;
    setOptions(ui.effort, efforts.map(function (x) { return { value: x, label: x }; }),
      agent && agent.effort ? agent.effort : (efforts[0] || ''));

    // UX-780: pickers are disabled while the agent works; Cancel shows instead.
    ui.mode.disabled = viewOnly || busy;
    ui.model.disabled = viewOnly || busy;
    ui.effort.disabled = viewOnly || busy;
    ui.agent.disabled = viewOnly;
    ui.cancel.hidden = !busy;
    ui.details.classList.toggle('btn-on', !!session.details_expanded);
    [ui.newConv, ui.status, ui.details, ui.share, ui.palette, ui.quit, ui.send].forEach(function (b) {
      b.disabled = viewOnly;
    });
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
        node.appendChild(el('span', 'msg-who'));
        node.appendChild(el('div', 'msg-text'));
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
      if (node.firstChild.textContent !== who) { node.firstChild.textContent = who; }
      // The agent color marks the message edge; the label keeps the theme's
      // text color so it stays readable for any configured agent color.
      var color = m.agent ? agentColor(m.agent) : null;
      node.style.borderLeftColor = color || '';
      if (node.lastChild.textContent !== m.content) { node.lastChild.textContent = m.content; }
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
    if (stickToBottom) { ui.transcript.scrollTop = ui.transcript.scrollHeight; }
  }

  function renderConversations() {
    var key = JSON.stringify(session.conversations) + session.conversation_id;
    if (ui.conversations.getAttribute('data-key') === key) { return; }
    ui.conversations.setAttribute('data-key', key);
    ui.conversations.textContent = '';
    session.conversations.forEach(function (c) {
      var li = el('li', 'session-conv' + (c.id === session.conversation_id ? ' session-conv-active' : ''));
      li.appendChild(el('span', 'session-conv-title', c.title || c.id.slice(0, 8)));
      if (c.id !== session.conversation_id) {
        var load = button('Load', 'resume', function () {
          command({ command: 'load_conversation', id: c.id });
        }, 'btn-small');
        load.disabled = viewOnly;
        li.appendChild(load);
      }
      ui.conversations.appendChild(li);
    });
  }

  // ---- palette ----------------------------------------------------------

  var ACTIONS = {
    '/agent': function () { ask('Agent', agentChoices(), function (v) { command({ command: 'select_agent', agent: v }); }); },
    '/mode': function () { ask('Mode', MODES.map(choice), function (v) { command({ command: 'set_mode', agent: session.selected_agent, mode: v }); }); },
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
    '/conversations': function () { ui.conversations.scrollIntoView({ block: 'nearest' }); },
    '/resume': function () {
      ask('Conversation', session.conversations.map(function (c) {
        return { value: c.id, label: (c.title || c.id.slice(0, 8)) + ' · ' + c.id.slice(0, 8) };
      }), function (v) { command({ command: 'load_conversation', id: v }); });
    },
    '/summarize': function () { askText('Focus (optional)', function (t) { sendPrompt(t ? '/summarize ' + t : '/summarize'); }); },
    '/compact': function () { askText('Focus (optional)', function (t) { sendPrompt(t ? '/compact ' + t : '/compact'); }); },
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
    '/compact': 'compact', '/share': 'share', '/image': 'image', '/details': 'details', '/graph': 'graph',
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
    var colors = {};
    (snap.agents || []).forEach(function (a) { if (a.color) { colors[a.id] = a.color; } });
    window.__dmuxSessionAgents = colors;
    viewOnly = snap.source === 'files';
    // `--web` publishes the session with its first snapshot; wait for it.
    root.hidden = !viewOnly && !snap.session;
    ui.viewOnly.hidden = !viewOnly;
    if (snap.session) {
      render(snap.session);
    } else if (viewOnly) {
      root.classList.add('session-disabled');
      session = null;
      [ui.agent, ui.mode, ui.model, ui.effort, ui.input, ui.send].forEach(function (c) { c.disabled = true; });
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
})();
