'use strict';

/* Read-only Markdown file path linkifier and viewer modal (FR-3416, FR-3417),
   with a chat docked beside it for board item files (SPEC-md-viewer-chat). */
(function () {
  var TOKEN_KEY = 'dmuxai.token';
  var SKIP_TAGS = { A: true, SCRIPT: true, STYLE: true, TEXTAREA: true, INPUT: true };
  var CHAT_POLL_MS = 3000;
  var CHAT_TIMEOUT_MS = 15 * 60 * 1000;

  var backdrop = null;
  var dialogCard = null;
  var titleEl = null;
  var closeBtn = null;
  var bodyEl = null;
  var mainEl = null;
  var lastTrigger = null;
  var requestSeq = 0;
  var currentPath = null;
  var currentShownPath = null;
  // FR-3840: chat state while the viewer shows a board item file; null otherwise.
  var chat = null;

  function currentToken() {
    try {
      return window.sessionStorage.getItem(TOKEN_KEY);
    } catch (err) {
      return null;
    }
  }

  function apiUrl(path) {
    if (typeof window.dmuxaiApiUrl === 'function') {
      return window.dmuxaiApiUrl(path);
    }
    return path;
  }

  function hasSkippedAncestor(node, rootEl) {
    var cur = node.parentNode;
    while (cur && cur.nodeType === 1) {
      if (SKIP_TAGS[cur.tagName]) {
        return true;
      }
      if (cur === rootEl) {
        break;
      }
      cur = cur.parentNode;
    }
    return false;
  }

  function isMdPathToken(clean) {
    if (!clean || clean.length < 4) { return false; }
    if (clean.indexOf('://') !== -1) { return false; }
    if (clean.indexOf('//') === 0) { return false; }
    if (/^[a-zA-Z][a-zA-Z0-9+.\-]*:/.test(clean)) { return false; }
    if (!/\.md$/i.test(clean)) { return false; }
    if (!/^(?:~\/|\/|[A-Za-z0-9_.])/.test(clean)) { return false; }
    var slash = clean.lastIndexOf('/');
    var base = slash >= 0 ? clean.slice(slash + 1) : clean;
    if (!base || base.length <= 3) { return false; }
    var stem = base.slice(0, -3);
    if (!stem || /^\.+$/.test(stem)) { return false; }
    return true;
  }

  function linkifyTextNode(node) {
    var text = node.nodeValue;
    if (!text || !/\.md/i.test(text)) { return; }
    var tokenRe = /[^\s<>"'`()\[\]{}]+/g;
    var frag = null;
    var lastIndex = 0;
    var match;
    while ((match = tokenRe.exec(text)) !== null) {
      var rawToken = match[0];
      var clean = rawToken.replace(/[.,;:!?]+$/, '');
      if (!isMdPathToken(clean)) { continue; }
      if (!frag) {
        frag = document.createDocumentFragment();
      }
      if (match.index > lastIndex) {
        frag.appendChild(document.createTextNode(text.slice(lastIndex, match.index)));
      }
      var link = document.createElement('a');
      link.href = '#';
      link.className = 'md-path-link';
      link.setAttribute('data-md-path', clean);
      link.textContent = clean;
      frag.appendChild(link);
      lastIndex = match.index + clean.length;
    }
    if (!frag) { return; }
    if (lastIndex < text.length) {
      frag.appendChild(document.createTextNode(text.slice(lastIndex)));
    }
    if (node.parentNode) {
      node.parentNode.replaceChild(frag, node);
    }
  }

  function linkifyMdPaths(rootEl) {
    if (!rootEl || typeof document === 'undefined' || !document.createTreeWalker) {
      return;
    }
    var walker = document.createTreeWalker(rootEl, NodeFilter.SHOW_TEXT, null);
    var textNodes = [];
    var current;
    while ((current = walker.nextNode()) !== null) {
      if (current.nodeValue && /\.md/i.test(current.nodeValue) && !hasSkippedAncestor(current, rootEl)) {
        textNodes.push(current);
      }
    }
    for (var i = 0; i < textNodes.length; i++) {
      linkifyTextNode(textNodes[i]);
    }
  }

  function ensureModal() {
    if (backdrop) { return; }

    backdrop = document.createElement('div');
    backdrop.id = 'md-viewer-backdrop';
    backdrop.className = 'md-viewer-backdrop';
    backdrop.hidden = true;

    dialogCard = document.createElement('div');
    dialogCard.id = 'md-viewer-dialog';
    dialogCard.className = 'md-viewer-card';
    dialogCard.setAttribute('role', 'dialog');
    dialogCard.setAttribute('aria-modal', 'true');
    dialogCard.setAttribute('aria-labelledby', 'md-viewer-title');

    var header = document.createElement('div');
    header.className = 'md-viewer-header';

    titleEl = document.createElement('h2');
    titleEl.id = 'md-viewer-title';
    titleEl.className = 'md-viewer-title';

    closeBtn = document.createElement('button');
    closeBtn.type = 'button';
    closeBtn.id = 'md-viewer-close';
    closeBtn.className = 'btn sidebar-close';
    closeBtn.setAttribute('aria-label', 'Close');
    closeBtn.textContent = '\u00D7';

    header.appendChild(titleEl);
    header.appendChild(closeBtn);

    bodyEl = document.createElement('div');
    bodyEl.id = 'md-viewer-body';
    bodyEl.className = 'md-viewer-body md-body';

    mainEl = document.createElement('div');
    mainEl.className = 'md-viewer-main';
    mainEl.appendChild(bodyEl);

    dialogCard.appendChild(header);
    dialogCard.appendChild(mainEl);
    backdrop.appendChild(dialogCard);
    document.body.appendChild(backdrop);

    closeBtn.addEventListener('click', function (e) {
      e.stopPropagation();
      closeViewer();
    });

    backdrop.addEventListener('click', function (e) {
      e.stopPropagation();
      if (e.target === backdrop) {
        closeViewer();
      }
    });

    dialogCard.addEventListener('click', function (e) {
      e.stopPropagation();
    });

    document.addEventListener('keydown', function (e) {
      if (e.key === 'Escape' && backdrop && !backdrop.hidden) {
        e.preventDefault();
        e.stopPropagation();
        if (typeof e.stopImmediatePropagation === 'function') {
          e.stopImmediatePropagation();
        }
        closeViewer();
      }
    }, true);
  }

  function closeViewer() {
    if (!backdrop || backdrop.hidden) { return; }
    requestSeq += 1;
    teardownChat();
    backdrop.hidden = true;
    document.body.classList.remove('md-viewer-open');
    var target = lastTrigger;
    lastTrigger = null;
    if (target && typeof target.focus === 'function' && target.isConnected) {
      try { target.focus(); } catch (err) { /* ignore */ }
    }
  }

  function showStatus(text, isError) {
    if (!bodyEl) { return; }
    bodyEl.textContent = '';
    var msg = document.createElement('p');
    msg.className = isError ? 'md-viewer-error' : 'md-viewer-status';
    msg.textContent = text;
    bodyEl.appendChild(msg);
  }

  function chatEl(tag, className, text) {
    var node = document.createElement(tag);
    if (className) { node.className = className; }
    if (text !== undefined && text !== null) { node.textContent = String(text); }
    return node;
  }

  // board.js answers only for links inside the open item modal in `--web`.
  function itemContextFor(trigger) {
    if (typeof window.dmuxaiMdViewerItemContext !== 'function') { return null; }
    var ctx = window.dmuxaiMdViewerItemContext(trigger);
    return ctx && ctx.itemId ? ctx : null;
  }

  function plannerCount(thread) {
    if (!Array.isArray(thread)) { return 0; }
    return thread.filter(function (m) { return m && m.from === 'planner'; }).length;
  }

  function pathCandidates() {
    var out = [];
    function add(p) {
      if (typeof p === 'string' && p && out.indexOf(p) === -1) { out.push(p); }
    }
    add(currentPath);
    add(currentShownPath);
    var ws = chat && chat.ctx.workspace ? String(chat.ctx.workspace).replace(/[\\/]+$/, '') : '';
    if (ws && currentPath && currentPath.indexOf(ws + '/') === 0) {
      add(currentPath.slice(ws.length + 1));
    }
    return out;
  }

  function renderChatThread() {
    if (!chat) { return; }
    var candidates = pathCandidates();
    var msgs = (chat.thread || []).filter(function (m) {
      if (!m || typeof m.text !== 'string') { return false; }
      return candidates.some(function (c) { return m.text.indexOf(c) !== -1; });
    });
    chat.threadEl.textContent = '';
    if (!msgs.length) {
      chat.threadEl.appendChild(chatEl('p', 'md-viewer-chat-empty', 'No messages about this file yet.'));
    }
    msgs.forEach(function (m) {
      var line = chatEl('div', 'md-viewer-chat-msg');
      if (m.from === 'owner') { line.classList.add('md-viewer-chat-msg-owner'); }
      line.appendChild(chatEl('div', 'md-viewer-chat-sender', m.from || 'unknown'));
      line.appendChild(chatEl('div', 'md-viewer-chat-bubble', m.text));
      chat.threadEl.appendChild(line);
    });
    chat.threadEl.scrollTop = chat.threadEl.scrollHeight;
  }

  function setChatStatus(text, isError) {
    if (!chat) { return; }
    chat.statusEl.textContent = text || '';
    chat.statusEl.hidden = !text;
    chat.statusEl.classList.toggle('md-viewer-chat-error', !!isError);
  }

  function setChatMode(mode) {
    if (!chat) { return; }
    chat.mode = mode === 'edit' ? 'edit' : 'ask';
    chat.askBtn.setAttribute('aria-pressed', chat.mode === 'ask' ? 'true' : 'false');
    chat.editBtn.setAttribute('aria-pressed', chat.mode === 'edit' ? 'true' : 'false');
    chat.input.placeholder = chat.mode === 'edit'
      ? 'Describe the change to make in this file'
      : 'Ask about this file';
  }

  function stopChatPoll(state) {
    if (state && state.pollTimer !== null) {
      window.clearInterval(state.pollTimer);
      state.pollTimer = null;
    }
  }

  function teardownChat() {
    if (!chat) { return; }
    stopChatPoll(chat);
    if (chat.panel.parentNode) { chat.panel.parentNode.removeChild(chat.panel); }
    if (dialogCard) { dialogCard.classList.remove('md-viewer-with-chat'); }
    chat = null;
  }

  function finishChat(state, item, error) {
    var wasEdit = !!(state.pending && state.pending.edit);
    state.pending = null;
    stopChatPoll(state);
    state.sendBtn.disabled = false;
    if (chat !== state) { return; }
    if (error) {
      setChatStatus(error, true);
      return;
    }
    state.thread = item.thread;
    renderChatThread();
    setChatStatus('', false);
    if (typeof state.ctx.onItem === 'function') { state.ctx.onItem(item); }
    // FR-3844: show the edited file.
    if (wasEdit && currentPath) { loadFile(currentPath); }
  }

  function startChatPoll(state) {
    stopChatPoll(state);
    var inFlight = false;
    state.pollTimer = window.setInterval(function () {
      if (chat !== state || !backdrop || backdrop.hidden || !state.pending) {
        stopChatPoll(state);
        return;
      }
      if (Date.now() - state.pending.startedAtMs >= CHAT_TIMEOUT_MS) {
        finishChat(state, null, 'No answer yet \u2014 check the thread later');
        return;
      }
      if (inFlight) { return; }
      inFlight = true;
      state.ctx.fetchJson('GET', '/api/board/' + encodeURIComponent(state.ctx.itemId)).then(function (item) {
        inFlight = false;
        if (chat !== state || !state.pending) { return; }
        if (item && plannerCount(item.thread) > state.pending.baseline) {
          finishChat(state, item, null);
        }
      }).catch(function () { inFlight = false; });
    }, CHAT_POLL_MS);
  }

  // FR-3841/FR-3842: Send runs only from the click/key handlers, so an edit
  // carries the human headers of a real user action.
  function chatSend() {
    var state = chat;
    if (!state || state.pending) { return; }
    var text = state.input.value.trim();
    if (!text) { return; }
    var edit = state.mode === 'edit';
    var body = { text: text, file: currentPath };
    if (!state.resumeSel.hidden) { body.resume = state.resumeSel.value; }
    if (edit) { body.mode = 'edit'; }
    state.pending = { edit: edit, baseline: plannerCount(state.thread), startedAtMs: Date.now() };
    state.sendBtn.disabled = true;
    setChatStatus(edit ? 'Editing in the work session\u2026' : 'Asking\u2026', false);
    state.ctx.fetchJson('POST', '/api/board/' + encodeURIComponent(state.ctx.itemId) + '/ask', body, edit)
      .then(function (item) {
        if (chat !== state) { return; }
        state.input.value = '';
        if (item && Array.isArray(item.thread)) {
          state.thread = item.thread;
          renderChatThread();
        }
        startChatPoll(state);
      })
      .catch(function (err) {
        state.pending = null;
        state.sendBtn.disabled = false;
        if (chat !== state) { return; }
        setChatStatus((err && err.message) ? err.message : 'Ask failed', true);
      });
  }

  function mountChat(ctx) {
    var panel = chatEl('aside', 'md-viewer-chat');
    panel.id = 'md-viewer-chat';
    panel.setAttribute('aria-label', 'Chat about this file');
    panel.appendChild(chatEl('div', 'md-viewer-chat-head', 'Chat about this file'));

    var threadEl = chatEl('div', 'md-viewer-chat-thread');
    threadEl.id = 'md-viewer-chat-thread';
    threadEl.setAttribute('role', 'log');
    panel.appendChild(threadEl);

    var modes = chatEl('div', 'md-viewer-chat-modes');
    modes.setAttribute('role', 'group');
    modes.setAttribute('aria-label', 'Chat mode');
    var askBtn = chatEl('button', 'btn btn-small md-viewer-chat-mode-btn', 'Ask');
    askBtn.type = 'button';
    askBtn.id = 'md-viewer-chat-ask';
    var editBtn = chatEl('button', 'btn btn-small md-viewer-chat-mode-btn', 'Edit');
    editBtn.type = 'button';
    editBtn.id = 'md-viewer-chat-edit';
    modes.appendChild(askBtn);
    modes.appendChild(editBtn);
    panel.appendChild(modes);

    var input = document.createElement('textarea');
    input.id = 'md-viewer-chat-input';
    input.className = 'md-viewer-chat-input';
    input.rows = 3;
    input.setAttribute('aria-label', 'Message about this file');
    panel.appendChild(input);

    var actions = chatEl('div', 'md-viewer-chat-actions');
    var resumeSel = document.createElement('select');
    resumeSel.id = 'md-viewer-chat-resume';
    resumeSel.className = 'md-viewer-chat-resume';
    resumeSel.setAttribute('aria-label', 'Session');
    (ctx.resumeOptions || []).forEach(function (opt) {
      var option = document.createElement('option');
      option.value = opt[0];
      option.textContent = opt[1];
      resumeSel.appendChild(option);
    });
    resumeSel.hidden = !ctx.hasWorkSession;
    actions.appendChild(resumeSel);
    var sendBtn = chatEl('button', 'btn btn-small btn-primary', 'Send');
    sendBtn.type = 'button';
    sendBtn.id = 'md-viewer-chat-send';
    actions.appendChild(sendBtn);
    panel.appendChild(actions);

    var statusEl = chatEl('div', 'md-viewer-chat-status');
    statusEl.id = 'md-viewer-chat-status';
    statusEl.setAttribute('role', 'status');
    statusEl.setAttribute('aria-live', 'polite');
    statusEl.hidden = true;
    panel.appendChild(statusEl);

    mainEl.appendChild(panel);
    dialogCard.classList.add('md-viewer-with-chat');
    chat = {
      ctx: ctx,
      thread: Array.isArray(ctx.thread) ? ctx.thread : [],
      panel: panel,
      threadEl: threadEl,
      input: input,
      askBtn: askBtn,
      editBtn: editBtn,
      resumeSel: resumeSel,
      sendBtn: sendBtn,
      statusEl: statusEl,
      mode: 'ask',
      pending: null,
      pollTimer: null
    };
    setChatMode('ask');
    askBtn.addEventListener('click', function () { setChatMode('ask'); });
    editBtn.addEventListener('click', function () { setChatMode('edit'); });
    sendBtn.addEventListener('click', function (e) {
      e.preventDefault();
      chatSend();
    });
    input.addEventListener('keydown', function (e) {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        chatSend();
      }
    });
    renderChatThread();
  }

  function openMdViewer(path, triggerEl) {
    if (typeof path !== 'string' || !path) { return; }
    ensureModal();
    lastTrigger = triggerEl || document.activeElement;
    currentPath = path;
    currentShownPath = null;
    titleEl.textContent = path;
    teardownChat();
    var ctx = itemContextFor(lastTrigger);
    if (ctx) { mountChat(ctx); }
    backdrop.hidden = false;
    document.body.classList.add('md-viewer-open');
    if (closeBtn && typeof closeBtn.focus === 'function') {
      try { closeBtn.focus(); } catch (err) { /* ignore */ }
    }
    loadFile(path);
  }

  function loadFile(path) {
    showStatus('Loading\u2026', false);
    var seq = ++requestSeq;
    var headers = { 'X-Dmuxai-Client': '1' };
    var token = currentToken();
    if (token) {
      headers.Authorization = 'Bearer ' + token;
    }

    var url = apiUrl('/api/md/view?path=' + encodeURIComponent(path));
    fetch(url, { method: 'GET', headers: headers, cache: 'no-store' })
      .then(function (res) {
        return res.text().then(function (text) {
          var payload = null;
          if (text) {
            try { payload = JSON.parse(text); } catch (err) { payload = null; }
          }
          if (!res.ok) {
            var errMsg = (payload && typeof payload.error === 'string')
              ? payload.error
              : ('HTTP ' + res.status);
            throw new Error(errMsg);
          }
          return payload;
        });
      })
      .then(function (payload) {
        if (seq !== requestSeq || !backdrop || backdrop.hidden) { return; }
        if (payload && typeof payload.path === 'string' && payload.path) {
          titleEl.textContent = payload.path;
          currentShownPath = payload.path;
          renderChatThread();
        }
        bodyEl.textContent = '';
        bodyEl.innerHTML = (payload && typeof payload.html === 'string') ? payload.html : '';
      })
      .catch(function (err) {
        if (seq !== requestSeq || !backdrop || backdrop.hidden) { return; }
        showStatus((err && err.message) ? err.message : 'Failed to load Markdown file', true);
      });
  }

  document.addEventListener('click', function (e) {
    if (e.defaultPrevented) { return; }
    var target = e.target;
    var link = target && target.closest ? target.closest('a.md-path-link') : null;
    if (!link) { return; }
    e.preventDefault();
    var path = link.getAttribute('data-md-path');
    if (path) {
      openMdViewer(path, link);
    }
  });

  window.dmuxaiLinkifyMdPaths = linkifyMdPaths;
  window.dmuxaiOpenMdViewer = openMdViewer;
})();
