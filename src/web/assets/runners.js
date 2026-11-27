'use strict';

// SPEC-runner-status FR-3311: on-demand runner status in the dashboard.
(function () {
  var apiUrl = window.dmuxaiApiUrl || function (p) { return p; };
  var TOKEN_KEY = 'dmuxai.token';
  var token = (function () {
    var fromQuery = new URLSearchParams(location.search).get('token');
    if (fromQuery) { return fromQuery; }
    try { return window.sessionStorage.getItem(TOKEN_KEY); } catch (err) { return null; }
  })();

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) { node.className = className; }
    if (text !== undefined) { node.textContent = text; }
    return node;
  }

  function api(method, path, body) {
    var headers = { 'X-Dmuxai-Client': '1' };
    if (token) { headers.Authorization = 'Bearer ' + token; }
    var init = { method: method, headers: headers, cache: 'no-store' };
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(body);
    }
    return fetch(apiUrl(path), init).then(function (res) {
      return res.json().catch(function () { return null; }).then(function (payload) {
        if (!res.ok) {
          throw new Error(payload && payload.error ? payload.error : 'HTTP ' + res.status);
        }
        return payload;
      });
    });
  }

  var toggle = document.getElementById('runners-toggle');
  var root = document.getElementById('runners-popover');
  if (!toggle || !root) { return; }

  var rows = [];
  var loading = false;
  var error = null;

  var BADGES = {
    ready: 'Ready',
    'not-logged-in': 'Not logged in',
    'not-installed': 'Not installed',
    unknown: 'Unknown',
    error: 'Error',
    disabled: 'Disabled',
    checking: 'Checking'
  };

  function badgeKind(row, isLoading) {
    if (isLoading) { return 'checking'; }
    if (row.enabled === false) { return 'disabled'; }
    if (row.installed === false) { return 'not-installed'; }
    if (row.auth === 'not_logged_in') { return 'not-logged-in'; }
    if (row.auth === 'error') { return 'error'; }
    if (row.auth === 'ok') { return 'ready'; }
    return 'unknown';
  }

  function badgeNode(kind) {
    return el('span', 'runner-badge runner-badge-' + kind, BADGES[kind]);
  }

  function replaceRow(updated) {
    var found = false;
    rows = rows.map(function (row) {
      if (row.id === updated.id) {
        found = true;
        return updated;
      }
      return row;
    });
    if (!found) { rows.push(updated); }
  }

  function renderRow(row) {
    var card = el('div', 'runner-row');
    card.setAttribute('data-runner', row.id || '');

    var head = el('div', 'runner-row-head');
    var name = el('div', 'runner-name');
    name.appendChild(el('span', 'runner-label', row.label || row.id || ''));
    if (row.id) { name.appendChild(el('span', 'runner-id', row.id)); }
    head.appendChild(name);
    head.appendChild(badgeNode(badgeKind(row, loading)));
    card.appendChild(head);

    var meta = el('div', 'runner-meta');
    if (row.version) {
      var version = el('span', 'runner-version', row.version);
      if (row.path) { version.title = row.path; }
      meta.appendChild(version);
    }
    if (row.account) { meta.appendChild(el('span', 'runner-account', row.account)); }
    if (meta.childNodes.length) { card.appendChild(meta); }

    if (row.hint) { card.appendChild(el('p', 'runner-hint', row.hint)); }
    if (row.error) { card.appendChild(el('p', 'memory-error', row.error)); }

    var switchRow = el('label', 'runner-switch');
    var input = el('input', 'runner-switch-input');
    input.type = 'checkbox';
    input.setAttribute('role', 'switch');
    input.setAttribute('aria-label', 'Enable ' + (row.label || row.id || 'runner'));
    input.checked = row.enabled !== false;
    input.disabled = loading;
    input.addEventListener('change', function () {
      var enabled = !!input.checked;
      input.disabled = true;
      api('POST', '/api/runners/enabled', { runner: row.id, enabled: enabled })
        .then(function (data) {
          if (data && data.runner) { replaceRow(data.runner); }
          error = null;
          render();
        })
        .catch(function (err) {
          input.checked = !enabled;
          error = String(err.message || err);
          render();
        });
    });
    switchRow.appendChild(input);
    switchRow.appendChild(el('span', 'runner-switch-label', 'Enabled'));
    card.appendChild(switchRow);

    return card;
  }

  function render() {
    root.replaceChildren();
    var header = el('div', 'runners-header');
    header.appendChild(el('h2', 'memory-title', 'Runners'));
    if (loading) {
      header.appendChild(badgeNode('checking'));
    }
    var refresh = el('button', 'btn btn-secondary btn-small', 'Refresh');
    refresh.type = 'button';
    refresh.disabled = loading;
    refresh.addEventListener('click', function () {
      loading = true;
      error = null;
      render();
      api('GET', '/api/runners/status?refresh=1').then(function (data) {
        rows = (data && data.runners) || [];
        loading = false;
        error = null;
        render();
      }).catch(function (err) {
        loading = false;
        error = String(err.message || err);
        render();
      });
    });
    header.appendChild(refresh);
    root.appendChild(header);

    if (error) { root.appendChild(el('p', 'memory-error', error)); }
    if (!rows.length && loading) {
      root.appendChild(el('p', 'memory-meta', 'Checking\u2026'));
    }

    var list = el('div', 'runners-list');
    rows.forEach(function (row) { list.appendChild(renderRow(row)); });
    root.appendChild(list);
  }

  function setOpen(open) {
    root.hidden = !open;
    toggle.setAttribute('aria-expanded', open ? 'true' : 'false');
    toggle.setAttribute('aria-pressed', open ? 'true' : 'false');
    toggle.classList.toggle('active', open);
  }

  toggle.addEventListener('click', function () {
    var open = root.hidden;
    setOpen(open);
    if (open) {
      loading = true;
      error = null;
      render();
      api('GET', '/api/runners/status').then(function (data) {
        rows = (data && data.runners) || [];
        loading = false;
        error = null;
        render();
      }).catch(function (err) {
        loading = false;
        error = String(err.message || err);
        render();
      });
      root.focus();
    }
  });
  document.addEventListener('keydown', function (event) {
    if (event.key === 'Escape' && !root.hidden) {
      setOpen(false);
    }
  });
})();
