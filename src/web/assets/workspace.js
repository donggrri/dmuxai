'use strict';
// SPEC-workspace-dir FR-871..FR-874, NFR-870: header workspace button and
// workspace switcher dialog (path input + live validation, MRU recent list,
// browse-root directory browser, and re-exec switch polling).

(function () {
  var TOKEN_KEY = 'dmuxai.token';
  var ELLIPSIS_MAX = 40;
  var VALIDATE_DEBOUNCE_MS = 120;
  var SWITCH_POLL_MS = 250;

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

  function abbreviateHome(fullPath, homeDir) {
    var p = String(fullPath || '');
    var h = String(homeDir || '');
    if (!p || !h) { return p; }
    if (p === h) { return '~'; }
    var prefix = h.endsWith('/') ? h : h + '/';
    if (p.indexOf(prefix) === 0) {
      return '~/' + p.slice(prefix.length);
    }
    return p;
  }

  function middleEllipsis(text, maxLen) {
    var s = String(text || '');
    var limit = typeof maxLen === 'number' && maxLen > 4 ? maxLen : ELLIPSIS_MAX;
    if (s.length <= limit) { return s; }
    var keep = limit - 1;
    var head = Math.ceil(keep / 2);
    var tail = Math.floor(keep / 2);
    return s.slice(0, head) + '\u2026' + s.slice(s.length - tail);
  }

  function formatWorkspaceLabel(fullPath, homeDir) {
    if (!fullPath || fullPath === '-' || fullPath === '\u2013') { return '\u2013'; }
    var home = typeof homeDir === 'string' && homeDir ? homeDir : (state && state.homeDir ? state.homeDir : '');
    return middleEllipsis(abbreviateHome(fullPath, home), ELLIPSIS_MAX);
  }

  window.dmuxaiMiddleEllipsis = middleEllipsis;
  window.dmuxaiFormatWorkspacePath = formatWorkspaceLabel;

  var state = {
    current: '',
    recent: [],
    browseRoots: [],
    homeDir: '',
    canSwitch: false,
    viewOnly: false,
    browsePath: '',
    browseParent: null,
    validPath: null,
    switching: false,
    validateTimer: null,
    validateSeq: 0,
    pollTimer: null
  };

  function $(id) {
    return document.getElementById(id);
  }

  function authHeaders(extra) {
    var h = extra ? Object.assign({}, extra) : {};
    if (token) {
      h['Authorization'] = 'Bearer ' + token;
    }
    return h;
  }

  function apiGet(path) {
    return fetch(path, {
      method: 'GET',
      headers: authHeaders()
    }).then(function (res) {
      return res.json().catch(function () { return {}; }).then(function (body) {
        return { status: res.status, ok: res.ok, body: body };
      });
    });
  }

  function apiPost(path, payload) {
    return fetch(path, {
      method: 'POST',
      headers: authHeaders({ 'Content-Type': 'application/json' }),
      body: JSON.stringify(payload || {})
    }).then(function (res) {
      return res.json().catch(function () { return {}; }).then(function (body) {
        return { status: res.status, ok: res.ok, body: body };
      });
    });
  }

  function updateHeaderButton() {
    var btn = $('workspace-button');
    if (!btn) { return; }
    var full = state.current || '';
    btn.title = full;
    btn.textContent = formatWorkspaceLabel(full, state.homeDir);
  }

  function updateSwitchButton() {
    var switchBtn = $('workspace-switch');
    if (switchBtn) {
      switchBtn.disabled = !state.canSwitch || state.viewOnly || !state.validPath || state.switching;
    }
    updateOpenButton();
  }

  function setReason(message, kind) {
    var el = $('workspace-reason');
    if (!el) { return; }
    if (!message) {
      el.hidden = true;
      el.textContent = '';
      el.className = 'workspace-reason';
      return;
    }
    el.hidden = false;
    el.textContent = String(message);
    el.className = 'workspace-reason' + (kind ? ' workspace-reason-' + kind : '');
  }

  function isWithinBrowseRoots(targetPath) {
    var p = String(targetPath || '');
    if (!p || !Array.isArray(state.browseRoots)) { return false; }
    for (var i = 0; i < state.browseRoots.length; i++) {
      var root = String(state.browseRoots[i] || '');
      if (!root) { continue; }
      if (p === root) { return true; }
      var prefix = root.endsWith('/') ? root : root + '/';
      if (p.indexOf(prefix) === 0) { return true; }
    }
    return false;
  }

  function renderRecent() {
    var list = $('workspace-recent');
    var section = $('workspace-recent-section');
    if (!list) { return; }
    while (list.firstChild) { list.removeChild(list.firstChild); }
    var items = Array.isArray(state.recent) ? state.recent : [];
    if (section) { section.hidden = items.length === 0; }
    for (var i = 0; i < items.length; i++) {
      (function (recentPath) {
        var li = document.createElement('li');
        li.className = 'workspace-recent-item';
        var btn = document.createElement('button');
        btn.type = 'button';
        btn.className = 'btn workspace-recent-btn';
        btn.title = recentPath;
        btn.textContent = abbreviateHome(recentPath, state.homeDir);
        btn.addEventListener('click', function () {
          var input = $('workspace-path');
          if (input) { input.value = recentPath; }
          validateNow(recentPath);
          if (!state.viewOnly && isWithinBrowseRoots(recentPath)) {
            loadDirs(recentPath);
          }
        });
        li.appendChild(btn);
        list.appendChild(li);
      })(String(items[i]));
    }
  }

  function renderDirs(path, parent, dirs, errorMsg) {
    state.browsePath = path || '';
    state.browseParent = parent || null;
    var cwdEl = $('workspace-browser-cwd');
    if (cwdEl) {
      cwdEl.textContent = abbreviateHome(state.browsePath, state.homeDir);
      cwdEl.title = state.browsePath;
    }
    var upBtn = $('workspace-up');
    if (upBtn) {
      upBtn.disabled = !state.browseParent || state.viewOnly || state.switching;
    }
    var list = $('workspace-browser');
    if (!list) { return; }
    while (list.firstChild) { list.removeChild(list.firstChild); }

    if (errorMsg) {
      var errLi = document.createElement('li');
      errLi.className = 'workspace-browser-empty';
      errLi.textContent = errorMsg;
      list.appendChild(errLi);
      return;
    }

    var items = Array.isArray(dirs) ? dirs : [];
    if (items.length === 0) {
      var emptyLi = document.createElement('li');
      emptyLi.className = 'workspace-browser-empty';
      emptyLi.textContent = 'No subdirectories';
      list.appendChild(emptyLi);
      return;
    }

    for (var i = 0; i < items.length; i++) {
      (function (dirName) {
        var li = document.createElement('li');
        li.className = 'workspace-browser-item';
        var btn = document.createElement('button');
        btn.type = 'button';
        btn.className = 'btn workspace-dir-btn';
        btn.textContent = dirName;
        var base = state.browsePath.endsWith('/')
          ? state.browsePath.slice(0, -1)
          : state.browsePath;
        var childPath = base ? base + '/' + dirName : '/' + dirName;
        btn.setAttribute('data-path', childPath);
        btn.addEventListener('click', function () {
          var input = $('workspace-path');
          if (input) { input.value = childPath; }
          validateNow(childPath);
          loadDirs(childPath);
        });
        li.appendChild(btn);
        list.appendChild(li);
      })(String(items[i]));
    }
  }

  function loadDirs(targetPath) {
    if (state.viewOnly) {
      renderDirs('', null, [], 'Directory browsing is disabled in view-only mode.');
      return;
    }
    var query = targetPath ? '?path=' + encodeURIComponent(targetPath) : '';
    apiGet('/api/workspace/dirs' + query).then(function (res) {
      if (!res.ok) {
        var msg = (res.body && res.body.error) ? res.body.error : ('HTTP ' + res.status);
        renderDirs(targetPath || '', null, [], msg);
        return;
      }
      renderDirs(res.body.path, res.body.parent, res.body.dirs, null);
    }).catch(function () {
      renderDirs(targetPath || '', null, [], 'Could not load directories.');
    });
  }

  function validateNow(rawPath) {
    if (state.validateTimer) {
      clearTimeout(state.validateTimer);
      state.validateTimer = null;
    }
    var trimmed = String(rawPath || '').trim();
    if (!trimmed) {
      state.validPath = null;
      setReason('does not exist', 'error');
      updateSwitchButton();
      return;
    }
    var seq = ++state.validateSeq;
    apiPost('/api/workspace/validate', { path: trimmed }).then(function (res) {
      if (seq !== state.validateSeq || state.switching) { return; }
      if (res.ok && res.body && res.body.ok) {
        state.validPath = String(res.body.path || trimmed);
        setReason('', '');
      } else {
        state.validPath = null;
        var reason = (res.body && (res.body.reason || res.body.error)) || 'invalid path';
        setReason(reason, 'error');
      }
      updateSwitchButton();
    }).catch(function () {
      if (seq !== state.validateSeq || state.switching) { return; }
      state.validPath = null;
      setReason('validation failed', 'error');
      updateSwitchButton();
    });
  }

  function scheduleValidate(rawPath) {
    if (state.validateTimer) {
      clearTimeout(state.validateTimer);
    }
    state.validPath = null;
    updateSwitchButton();
    state.validateTimer = setTimeout(function () {
      validateNow(rawPath);
    }, VALIDATE_DEBOUNCE_MS);
  }

  function fetchWorkspaceInfo(onDone) {
    if (!token) {
      if (onDone) { onDone(); }
      return;
    }
    apiGet('/api/workspace').then(function (res) {
      if (res.ok && res.body) {
        if (typeof res.body.current === 'string' && res.body.current) {
          state.current = res.body.current;
        }
        state.recent = Array.isArray(res.body.recent) ? res.body.recent : [];
        state.browseRoots = Array.isArray(res.body.browse_roots) ? res.body.browse_roots : [];
        state.homeDir = state.browseRoots.length > 0 ? String(state.browseRoots[0]) : state.homeDir;
        state.canSwitch = !!res.body.can_switch;
        state.viewOnly = !state.canSwitch && state.viewOnly;
        updateHeaderButton();
        try { window.dispatchEvent(new Event('dmuxai:workspace-info')); } catch (err) { /* ignore */ }
      }
      if (onDone) { onDone(); }
    }).catch(function () {
      if (onDone) { onDone(); }
    });
  }

  function openDialog() {
    var dialog = $('workspace-dialog');
    if (!dialog) { return; }
    state.switching = false;
    dialog.hidden = false;

    fetchWorkspaceInfo(function () {
      var curEl = $('workspace-current');
      if (curEl) { curEl.textContent = state.current || '\u2013'; }
      var viewHint = $('workspace-view-only');
      if (viewHint) { viewHint.hidden = !state.viewOnly; }
      var input = $('workspace-path');
      if (input) {
        input.value = state.current || '';
        input.focus();
      }
      renderRecent();
      if (state.current) {
        validateNow(state.current);
      } else {
        setReason('', '');
        updateSwitchButton();
      }
      var startDir = isWithinBrowseRoots(state.current)
        ? state.current
        : (state.browseRoots[0] || state.current || '');
      loadDirs(startDir);
    });
  }

  function closeDialog() {
    var dialog = $('workspace-dialog');
    if (!dialog || dialog.hidden) { return; }
    if (state.pollTimer) {
      clearInterval(state.pollTimer);
      state.pollTimer = null;
    }
    state.switching = false;
    dialog.hidden = true;
    var btn = $('workspace-button');
    if (btn) { btn.focus(); }
  }

  function startSwitchPoll(targetPath, previousCurrent) {
    if (state.pollTimer) {
      clearInterval(state.pollTimer);
    }
    state.pollTimer = setInterval(function () {
      apiGet('/api/workspace').then(function (res) {
        if (res.ok && res.body && typeof res.body.current === 'string') {
          if (res.body.current === targetPath || (previousCurrent && res.body.current !== previousCurrent)) {
            clearInterval(state.pollTimer);
            state.pollTimer = null;
            window.location.reload();
          }
        }
      }).catch(function () {
        // Server is restarting during exec; keep polling.
      });
    }, SWITCH_POLL_MS);
  }

  function submitSwitch() {
    if (state.switching || !state.canSwitch || state.viewOnly) { return; }
    var input = $('workspace-path');
    var target = state.validPath || (input ? input.value.trim() : '');
    if (!target) { return; }

    var previousCurrent = state.current;
    state.switching = true;
    updateSwitchButton();
    updateOpenButton();
    setReason('Switching\u2026', 'info');

    apiPost('/api/workspace/switch', { path: target }).then(function (res) {
      if (res.status === 202) {
        var canonical = (res.body && res.body.path) ? String(res.body.path) : target;
        setReason('Switching\u2026', 'info');
        startSwitchPoll(canonical, previousCurrent);
        return;
      }
      state.switching = false;
      var errMsg = (res.body && (res.body.error || res.body.reason)) || ('HTTP ' + res.status);
      setReason(errMsg, 'error');
      updateSwitchButton();
      updateOpenButton();
    }).catch(function () {
      state.switching = false;
      setReason('Workspace switch request failed.', 'error');
      updateSwitchButton();
      updateOpenButton();
    });
  }

  function updateOpenButton() {
    var btn = $('workspace-open');
    if (!btn) { return; }
    var input = $('workspace-path');
    var target = state.validPath || (input ? input.value.trim() : '');
    btn.disabled = state.viewOnly || state.switching || !target;
  }

  function submitOpen() {
    if (state.switching || state.viewOnly) { return; }
    var input = $('workspace-path');
    var target = state.validPath || (input ? input.value.trim() : '');
    if (!target) { return; }
    state.switching = true;
    updateSwitchButton();
    updateOpenButton();
    setReason('Opening\u2026', 'info');
    apiPost('/api/workspace/open', { path: target }).then(function (res) {
      state.switching = false;
      updateSwitchButton();
      updateOpenButton();
      if (res.ok && res.body && res.body.url) {
        window.open(String(res.body.url), '_blank', 'noopener');
        setReason('Opened another dashboard. This window stays on the current workspace.', 'info');
        return;
      }
      var errMsg = (res.body && (res.body.error || res.body.reason)) || ('HTTP ' + res.status);
      setReason(errMsg, 'error');
    }).catch(function () {
      state.switching = false;
      updateSwitchButton();
      updateOpenButton();
      setReason('Could not open another dashboard.', 'error');
    });
  }

  // SPEC-dashboard-fullwidth FR-1227: the Sessions sidebar opens a
  // conversation of another workspace through the same switch + poll flow.
  window.dmuxaiWorkspaceCanSwitch = function () {
    return !!state.canSwitch && !state.viewOnly;
  };
  window.dmuxaiSwitchWorkspace = function (path) {
    return new Promise(function (resolve) {
      fetchWorkspaceInfo(function () {
        if (!state.canSwitch || state.viewOnly) {
          resolve({ ok: false, error: 'Switching workspace is not available in this dashboard.' });
          return;
        }
        if (state.switching) {
          resolve({ ok: false, error: 'A workspace switch is already in progress.' });
          return;
        }
        var previousCurrent = state.current;
        state.switching = true;
        apiPost('/api/workspace/switch', { path: path }).then(function (res) {
          if (res.status === 202) {
            var canonical = (res.body && res.body.path) ? String(res.body.path) : path;
            startSwitchPoll(canonical, previousCurrent);
            resolve({ ok: true });
            return;
          }
          state.switching = false;
          resolve({ ok: false, error: (res.body && (res.body.error || res.body.reason)) || ('HTTP ' + res.status) });
        }).catch(function () {
          state.switching = false;
          resolve({ ok: false, error: 'Workspace switch request failed.' });
        });
      });
    });
  };

  function init() {
    var btn = $('workspace-button');
    if (btn) {
      btn.addEventListener('click', openDialog);
    }
    var closeBtn = $('workspace-close');
    if (closeBtn) {
      closeBtn.addEventListener('click', closeDialog);
    }
    var cancelBtn = $('workspace-cancel');
    if (cancelBtn) {
      cancelBtn.addEventListener('click', closeDialog);
    }
    var openBtn = $('workspace-open');
    if (openBtn) {
      openBtn.addEventListener('click', submitOpen);
    }
    var dialog = $('workspace-dialog');
    if (dialog) {
      dialog.addEventListener('click', function (e) {
        if (e.target === dialog) { closeDialog(); }
      });
    }
    var input = $('workspace-path');
    if (input) {
      input.addEventListener('input', function () {
        scheduleValidate(input.value);
      });
      input.addEventListener('keydown', function (e) {
        if (e.key === 'Enter') {
          e.preventDefault();
          validateNow(input.value);
        }
      });
    }
    var upBtn = $('workspace-up');
    if (upBtn) {
      upBtn.addEventListener('click', function () {
        if (state.browseParent) {
          var parentPath = state.browseParent;
          var pathInput = $('workspace-path');
          if (pathInput) { pathInput.value = parentPath; }
          validateNow(parentPath);
          loadDirs(parentPath);
        }
      });
    }
    var switchBtn = $('workspace-switch');
    if (switchBtn) {
      switchBtn.addEventListener('click', submitSwitch);
    }
    document.addEventListener('keydown', function (e) {
      if (e.key === 'Escape') {
        var dlg = $('workspace-dialog');
        if (dlg && !dlg.hidden) {
          e.preventDefault();
          closeDialog();
        }
      }
    });

    window.addEventListener('dmuxai:snapshot', function (ev) {
      var snap = ev && ev.detail;
      if (!snap) { return; }
      if (typeof snap.workspace === 'string' && snap.workspace) {
        state.current = snap.workspace;
      }
      state.viewOnly = snap.source === 'files';
      updateHeaderButton();
    });

    fetchWorkspaceInfo();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
