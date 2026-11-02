'use strict';

// SPEC-jira-todos UX-2409..UX-2411: read-only Jira group in the To-dos panel.
(function () {
  var apiUrl = window.dmuxaiApiUrl || function (p) { return p; };
  var TOKEN_KEY = 'dmuxai.token';
  var COLLAPSED_KEY = 'dmuxai.jiraCollapsed';
  var MIN_FETCH_MS = 60000;
  var token = (function () {
    var fromQuery = new URLSearchParams(location.search).get('token');
    if (fromQuery) { return fromQuery; }
    try { return window.sessionStorage.getItem(TOKEN_KEY); } catch (err) { return null; }
  })();

  var group = document.getElementById('jira-group');
  var toggle = document.getElementById('jira-toggle');
  var count = document.getElementById('jira-count');
  var refresh = document.getElementById('jira-refresh');
  var body = document.getElementById('jira-body');
  var hint = document.getElementById('jira-hint');
  var banner = document.getElementById('jira-banner');
  var list = document.getElementById('jira-list');
  if (!group || !toggle || !count || !refresh || !body || !hint || !banner || !list) {
    return;
  }

  var lastFetchAt = 0;
  var inFlight = false;

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) { node.className = className; }
    if (text !== undefined) { node.textContent = text; }
    return node;
  }

  function isViewOnly() {
    return typeof window.isDashboardViewOnly === 'function' && window.isDashboardViewOnly();
  }

  function request(method, path, payload) {
    var headers = { Authorization: 'Bearer ' + token };
    var init = { method: method, headers: headers, cache: 'no-store' };
    if (payload !== undefined) {
      headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(payload);
    }
    return fetch(apiUrl(path), init).then(function (res) {
      return res.text().then(function (text) {
        var data = null;
        if (text) {
          try { data = JSON.parse(text); } catch (err) { data = null; }
        }
        if (!res.ok) {
          var message = data && data.error;
          if (message && typeof message === 'object') { message = message.message; }
          throw new Error(typeof message === 'string' ? message : 'HTTP ' + res.status);
        }
        return data;
      });
    });
  }

  function showBanner(message) {
    banner.textContent = message ? String(message) : '';
    banner.hidden = !message;
  }

  function safeUrl(raw) {
    var value = String(raw || '');
    if (value.indexOf('https://') === 0) { return value; }
    if (value.indexOf('http://127.0.0.1') === 0 || value.indexOf('http://localhost') === 0) {
      return value;
    }
    return null;
  }

  function startTitle() {
    return 'Start dmuxai --web to start tasks';
  }

  function syncViewOnly() {
    var disabled = isViewOnly();
    var buttons = list.querySelectorAll('.jira-start');
    for (var i = 0; i < buttons.length; i++) {
      buttons[i].disabled = disabled;
      if (disabled) {
        buttons[i].title = startTitle();
      } else {
        buttons[i].removeAttribute('title');
      }
    }
  }

  function issueItem(issue) {
    var item = el('li', 'jira-item');
    item.appendChild(el('span', 'jira-key badge', String(issue.key || '')));
    item.appendChild(el('span', 'jira-summary', String(issue.summary || '')));
    var status = el('span', 'jira-status chip', String(issue.status || ''));
    status.setAttribute('data-category', String(issue.status_category || ''));
    item.appendChild(status);
    var actions = el('span', 'jira-actions');

    var open = el('button', 'btn jira-open', 'Open in Jira');
    open.type = 'button';
    open.addEventListener('click', function () {
      var url = safeUrl(issue.url);
      if (url) { window.open(url, '_blank', 'noopener,noreferrer'); }
    });
    actions.appendChild(open);

    var start = el('button', 'btn jira-start', 'Start task');
    start.type = 'button';
    start.disabled = isViewOnly();
    if (start.disabled) { start.title = startTitle(); }
    start.addEventListener('click', function () {
      if (isViewOnly() || typeof window.dmuxaiStartDraft !== 'function') { return; }
      window.dmuxaiStartDraft(String(issue.key || '') + ' ' + String(issue.summary || ''));
    });
    actions.appendChild(start);

    var copy = el('button', 'btn jira-copy', 'Copy to to-do');
    copy.type = 'button';
    copy.addEventListener('click', function () {
      var title = (String(issue.key || '') + ' ' + String(issue.summary || '')).slice(0, 200);
      request('POST', '/api/todos', { title: title, external_key: issue.key })
        .then(function () {
          showBanner('');
          copy.textContent = 'Copied';
          copy.disabled = true;
          window.setTimeout(function () {
            copy.textContent = 'Copy to to-do';
            copy.disabled = false;
          }, 1200);
        })
        .catch(function (err) { showBanner(err && err.message ? err.message : String(err)); });
    });
    actions.appendChild(copy);

    item.appendChild(actions);
    return item;
  }

  function render(data) {
    group.hidden = false;
    list.replaceChildren();
    showBanner('');
    if (!data || data.configured === false) {
      hint.hidden = false;
      refresh.hidden = true;
      count.textContent = '0';
      return;
    }
    hint.hidden = true;
    refresh.hidden = false;
    if (data.error && data.error.message) { showBanner(data.error.message); }
    var issues = Array.isArray(data.issues) ? data.issues : [];
    count.textContent = String(issues.length);
    for (var i = 0; i < issues.length; i++) {
      list.appendChild(issueItem(issues[i] || {}));
    }
    syncViewOnly();
  }

  function renderError(err) {
    group.hidden = false;
    hint.hidden = true;
    refresh.hidden = false;
    count.textContent = '0';
    list.replaceChildren();
    showBanner(err && err.message ? err.message : String(err));
  }

  function load(force) {
    if (!token || inFlight) { return; }
    var now = Date.now();
    if (!force && lastFetchAt && now - lastFetchAt < MIN_FETCH_MS) { return; }
    lastFetchAt = now;
    inFlight = true;
    var path = '/api/jira/issues' + (force ? '?refresh=1' : '');
    request('GET', path)
      .then(function (data) { inFlight = false; render(data); })
      .catch(function (err) { inFlight = false; renderError(err); });
  }

  function setExpanded(expanded, remember) {
    toggle.setAttribute('aria-expanded', expanded ? 'true' : 'false');
    body.hidden = !expanded;
    var caret = toggle.querySelector('.jira-caret');
    if (caret) { caret.textContent = expanded ? '\u25be' : '\u25b8'; }
    if (remember) {
      try { window.localStorage.setItem(COLLAPSED_KEY, expanded ? '0' : '1'); } catch (err) { /* ignore */ }
    }
  }

  var collapsed = false;
  try { collapsed = window.localStorage.getItem(COLLAPSED_KEY) === '1'; } catch (err) { /* ignore */ }
  setExpanded(!collapsed, false);

  toggle.addEventListener('click', function () {
    setExpanded(body.hidden, true);
  });
  refresh.addEventListener('click', function () { load(true); });

  var tab = document.getElementById('tab-todos');
  if (tab) {
    tab.addEventListener('click', function () { load(false); });
    tab.addEventListener('keydown', function (event) {
      if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') { return; }
      window.setTimeout(function () {
        if (tab.getAttribute('aria-selected') === 'true') { load(false); }
      }, 0);
    });
  }
  var todosToggle = document.getElementById('todos-toggle');
  if (todosToggle) { todosToggle.addEventListener('click', function () { load(false); }); }

  document.addEventListener('dmuxai:snapshot', syncViewOnly);
  load(false);
})();
