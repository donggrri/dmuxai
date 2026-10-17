'use strict';

(function () {
  var REFRESH_MS = 15000;
  var TOKEN_KEY = 'dmuxai.token';
  var NAME_RE = /^[a-z0-9][a-z0-9_-]{0,47}$/;
  var TEXT_MAX = 200;
  var NO_TOKEN_MSG =
    'Open the dashboard URL printed by dmuxai (with ?token=…)';
  var MEMORY_OFF_MSG =
    'Memory is off. Set [memory] vault = "~/Obsidian/Vault" in ~/.config/dmuxai/dmuxai.toml';

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

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) { node.className = className; }
    if (text !== undefined) { node.textContent = text; }
    return node;
  }

  function api(method, path, body) {
    var headers = { Authorization: 'Bearer ' + token };
    var init = { method: method, headers: headers, cache: 'no-store' };
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(body);
    }
    return fetch(path, init).then(function (res) {
      return res.text().then(function (text) {
        var payload = null;
        if (text) {
          try { payload = JSON.parse(text); } catch (err) { payload = null; }
        }
        if (!res.ok) {
          var message = payload && typeof payload.error === 'string'
            ? payload.error
            : 'HTTP ' + res.status;
          var err = new Error(message);
          err.status = res.status;
          throw err;
        }
        return payload;
      });
    });
  }

  var root = document.getElementById('memory-panel');
  if (!root) { return; }

  var state = {
    project: '',
    projectTarget: '',
    pending: [],
    context: ''
  };

  /** @type {'none'|'no-token'|'disabled'|'enabled'} */
  var uiMode = 'none';
  var inFlight = 0;

  var statusEl = null;
  var nameField = null;
  var nameInput = null;
  var targetSelect = null;
  var textInput = null;
  var metaEl = null;
  var pendingMount = null;
  var contextDetails = null;
  var contextPre = null;
  var saveBtn = null;
  var refreshTimer = null;

  function setBusy() {
    var busy = inFlight > 0;
    if (saveBtn) { saveBtn.disabled = busy; }
    if (!pendingMount) { return; }
    var buttons = pendingMount.querySelectorAll('.memory-action-btn');
    for (var i = 0; i < buttons.length; i++) {
      buttons[i].disabled = busy;
    }
  }

  function withRequest(promise) {
    inFlight += 1;
    setBusy();
    return promise.finally(function () {
      inFlight -= 1;
      setBusy();
    });
  }

  function setStatus(message, isError) {
    if (!statusEl) { return; }
    statusEl.textContent = message || '';
    statusEl.classList.toggle('memory-status-error', !!isError);
  }

  function targetKindNeedsName(kind) {
    return kind === 'decision' || kind === 'gotcha';
  }

  function buildTarget() {
    var kind = targetSelect ? targetSelect.value : 'project';
    if (kind === 'human') { return 'human.md'; }
    if (kind === 'project') { return state.projectTarget; }
    var name = nameInput ? nameInput.value.trim() : '';
    if (!NAME_RE.test(name)) {
      throw new Error('invalid target name (use a-z, 0-9, _, -)');
    }
    if (kind === 'decision') { return 'decisions/' + name + '.md'; }
    return 'gotchas/' + name + '.md';
  }

  function onTargetChange() {
    if (!nameField || !targetSelect) { return; }
    var show = targetKindNeedsName(targetSelect.value);
    nameField.hidden = !show;
  }

  function renderNoToken() {
    root.textContent = '';
    statusEl = null;
    metaEl = null;
    pendingMount = null;
    contextDetails = null;
    contextPre = null;
    saveBtn = null;
    nameField = null;
    nameInput = null;
    targetSelect = null;
    textInput = null;
    uiMode = 'no-token';
    root.appendChild(el('h2', 'panel-title', 'Memory'));
    root.appendChild(el('p', 'memory-off', NO_TOKEN_MSG));
  }

  function renderMemoryOff() {
    root.textContent = '';
    statusEl = null;
    metaEl = null;
    pendingMount = null;
    contextDetails = null;
    contextPre = null;
    saveBtn = null;
    nameField = null;
    nameInput = null;
    targetSelect = null;
    textInput = null;
    uiMode = 'disabled';
    root.appendChild(el('h2', 'panel-title', 'Memory'));
    root.appendChild(el('p', 'memory-off', MEMORY_OFF_MSG));
  }

  function proposalAction(path, busyLabel, doneLabel) {
    setStatus(busyLabel, false);
    return withRequest(
      api('POST', path)
        .then(function (payload) {
          setStatus(
            payload && payload.message ? String(payload.message) : doneLabel,
            false
          );
          return refresh();
        })
        .catch(function (err) {
          setStatus(err.message, true);
        })
    );
  }

  function fillPendingList(list) {
    if (!pendingMount) { return; }
    pendingMount.textContent = '';
    pendingMount.appendChild(el('h3', 'memory-pending-title', 'Pending proposals'));
    if (!list.length) {
      pendingMount.appendChild(el('p', 'memory-meta', 'No pending proposals.'));
      return;
    }
    var ul = el('ul', 'memory-pending-list');
    for (var i = 0; i < list.length; i++) {
      (function (item) {
        var li = el('li', 'memory-proposal');
        li.appendChild(el('p', 'memory-proposal-target', String(item.target || '')));
        li.appendChild(el('p', 'memory-proposal-text', String(item.text || '')));
        var actions = el('div', 'memory-proposal-actions');
        var approveBtn = el('button', 'btn btn-primary memory-action-btn', 'Approve');
        approveBtn.type = 'button';
        approveBtn.addEventListener('click', function () {
          var id = String(item.id || '');
          proposalAction(
            '/api/memory/proposals/' + encodeURIComponent(id) + '/approve',
            'Approving…',
            'Approved.'
          );
        });
        var rejectBtn = el('button', 'btn memory-action-btn', 'Reject');
        rejectBtn.type = 'button';
        rejectBtn.addEventListener('click', function () {
          var id = String(item.id || '');
          proposalAction(
            '/api/memory/proposals/' + encodeURIComponent(id) + '/reject',
            'Rejecting…',
            'Rejected.'
          );
        });
        actions.appendChild(approveBtn);
        actions.appendChild(rejectBtn);
        li.appendChild(actions);
        ul.appendChild(li);
      })(list[i]);
    }
    pendingMount.appendChild(ul);
    setBusy();
  }

  function mountEnabledShell() {
    root.textContent = '';
    uiMode = 'enabled';

    root.appendChild(el('h2', 'panel-title', 'Memory'));
    metaEl = el('p', 'memory-meta');
    root.appendChild(metaEl);

    var controlsWrap = el('div', 'memory-controls');

    var targetRow = el('div', 'memory-target-row');
    var targetField = el('label', 'memory-field');
    targetField.appendChild(el('span', 'field-label', 'Target'));
    targetSelect = el('select', 'memory-target-select');
    [
      ['project', 'Project'],
      ['human', 'User (human.md)'],
      ['decision', 'Decision'],
      ['gotcha', 'Gotcha']
    ].forEach(function (pair) {
      var opt = el('option', '', pair[1]);
      opt.value = pair[0];
      targetSelect.appendChild(opt);
    });
    targetSelect.addEventListener('change', onTargetChange);
    targetField.appendChild(targetSelect);
    targetRow.appendChild(targetField);

    nameField = el('label', 'memory-field');
    nameField.appendChild(el('span', 'field-label', 'Name'));
    nameInput = el('input', 'memory-name-input');
    nameInput.type = 'text';
    nameInput.maxLength = 48;
    nameInput.autocomplete = 'off';
    nameField.appendChild(nameInput);
    nameField.hidden = true;
    targetRow.appendChild(nameField);
    controlsWrap.appendChild(targetRow);

    var saveRow = el('div', 'memory-save-row');
    var textField = el('label', 'memory-field');
    textField.appendChild(el('span', 'field-label', 'Entry'));
    textInput = el('input', 'memory-text-input');
    textInput.type = 'text';
    textInput.maxLength = TEXT_MAX;
    textInput.autocomplete = 'off';
    textField.appendChild(textInput);
    saveRow.appendChild(textField);
    saveBtn = el('button', 'btn btn-primary memory-action-btn', 'Save');
    saveBtn.type = 'button';
    saveBtn.addEventListener('click', onSaveClick);
    saveRow.appendChild(saveBtn);
    controlsWrap.appendChild(saveRow);

    statusEl = el('p', 'memory-status');
    controlsWrap.appendChild(statusEl);
    root.appendChild(controlsWrap);

    pendingMount = el('div', 'memory-pending');
    root.appendChild(pendingMount);

    contextDetails = el('details', 'memory-context-details');
    contextDetails.appendChild(el('summary', '', 'Injected context preview'));
    contextPre = el('pre', 'memory-context-pre');
    contextDetails.appendChild(contextPre);
    root.appendChild(contextDetails);

    onTargetChange();
  }

  function onSaveClick() {
    var text = textInput ? textInput.value.trim() : '';
    if (!text) {
      setStatus('Entry is empty', true);
      return;
    }
    var target;
    try {
      target = buildTarget();
    } catch (err) {
      setStatus(err.message, true);
      return;
    }
    setStatus('Saving…', false);
    withRequest(
      api('POST', '/api/memory/save', { text: text, target: target })
        .then(function (payload) {
          if (payload && payload.message) {
            setStatus(String(payload.message), false);
          }
          if (textInput) { textInput.value = ''; }
          return refresh();
        })
        .catch(function (err) {
          setStatus(err.message, true);
        })
    );
  }

  function applyEnabledData(data) {
    var project = String(data.project || '');
    state.project = project;
    state.projectTarget = 'projects/' + project + '.md';
    state.context = String(data.context || '');
    state.pending = Array.isArray(data.pending) ? data.pending : [];

    if (metaEl) {
      metaEl.textContent = 'Project: ' + project + ' · dir: ' + String(data.dir || '');
    }
    fillPendingList(state.pending);
    if (contextPre) {
      contextPre.textContent = state.context || '(empty)';
    }
  }

  function refresh() {
    if (!token) {
      if (uiMode !== 'no-token') {
        renderNoToken();
      }
      return Promise.resolve();
    }
    return api('GET', '/api/memory')
      .then(function (data) {
        if (!data || data.enabled !== true) {
          if (uiMode !== 'disabled') {
            renderMemoryOff();
          }
          return;
        }
        if (uiMode !== 'enabled') {
          mountEnabledShell();
        }
        applyEnabledData(data);
      })
      .catch(function (err) {
        if (err.status === 401) {
          if (uiMode !== 'enabled') {
            renderNoToken();
          }
          setStatus('Unauthorized — open the dashboard URL with ?token=…', true);
          return;
        }
        setStatus(err.message, true);
      });
  }

  function tickRefresh() {
    if (document.hidden) { return; }
    refresh();
  }

  function start() {
    if (!token) {
      renderNoToken();
    } else {
      refresh();
    }
    if (refreshTimer) { window.clearInterval(refreshTimer); }
    refreshTimer = window.setInterval(tickRefresh, REFRESH_MS);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start);
  } else {
    start();
  }
})();
