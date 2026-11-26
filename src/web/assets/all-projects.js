'use strict';

/* SPEC-web-all-projects FR-2984..FR-2985: All projects global view (read-only + Open project). */
(function () {
  var TOKEN_KEY = 'dmuxai.token';
  var open = false;
  var projects = [];
  var selectedRepoKey = null;
  var selectedConvId = null;
  var selectedRunId = null;
  var canOpenInWeb = false;
  var viewOnlyMode = true;
  var activeStream = null;

  function $(id) {
    return document.getElementById(id);
  }

  function getToken() {
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

  function apiUrlFor(wsId, path) {
    if (typeof window.dmuxaiApiUrlFor === 'function') {
      return window.dmuxaiApiUrlFor(wsId, path);
    }
    var url = new URL(path, window.location.origin);
    if (wsId && wsId !== 'default') {
      url.searchParams.set('ws', wsId);
    }
    return url.pathname + url.search;
  }

  function authHeaders(extra) {
    var h = extra ? Object.assign({}, extra) : {};
    h['X-Dmuxai-Client'] = '1';
    var tok = getToken();
    if (tok) {
      h['Authorization'] = 'Bearer ' + tok;
    }
    return h;
  }

  function fetchJson(url) {
    return fetch(url, {
      method: 'GET',
      headers: authHeaders(),
      cache: 'no-store'
    }).then(function (res) {
      if (!res.ok) {
        throw new Error('HTTP ' + res.status);
      }
      return res.json();
    });
  }

  function postJson(url, body) {
    return fetch(url, {
      method: 'POST',
      headers: authHeaders({ 'Content-Type': 'application/json' }),
      body: JSON.stringify(body || {})
    }).then(function (res) {
      return res.json().catch(function () { return {}; }).then(function (data) {
        return { ok: res.ok, status: res.status, body: data };
      });
    });
  }

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) { node.className = className; }
    if (text !== undefined && text !== null) {
      node.textContent = String(text);
    }
    return node;
  }

  function closeStream() {
    if (activeStream) {
      try { activeStream.close(); } catch (err) { /* ignore */ }
      activeStream = null;
    }
  }

  function selectedProject() {
    for (var i = 0; i < projects.length; i++) {
      if (projects[i].repo_key === selectedRepoKey) {
        return projects[i];
      }
    }
    return null;
  }

  function updateOpenButton() {
    var btn = $('all-projects-open-btn');
    var hint = $('all-projects-open-hint');
    if (!btn) { return; }
    var proj = selectedProject();
    if (!proj) {
      btn.disabled = true;
      if (hint) { hint.textContent = ''; hint.hidden = true; }
      return;
    }
    if (viewOnlyMode || !canOpenInWeb) {
      btn.disabled = true;
      if (hint) {
        hint.textContent = 'View-only: start dmuxai --web to open projects.';
        hint.hidden = false;
      }
      return;
    }
    if (proj.missing || !proj.canonical_path) {
      btn.disabled = true;
      if (hint) {
        hint.textContent = 'Folder is missing on disk.';
        hint.hidden = false;
      }
      return;
    }
    btn.disabled = false;
    if (hint) { hint.textContent = ''; hint.hidden = true; }
  }

  function renderProjectsList() {
    var list = $('all-projects-list');
    var empty = $('all-projects-empty');
    if (!list) { return; }
    list.replaceChildren();
    if (!projects.length) {
      if (empty) { empty.hidden = false; }
      return;
    }
    if (empty) { empty.hidden = true; }

    projects.forEach(function (proj) {
      var li = el('li', 'all-projects-item');
      if (proj.repo_key === selectedRepoKey) {
        li.classList.add('selected');
      }
      var btn = el('button', 'all-projects-item-btn');
      btn.type = 'button';
      btn.setAttribute('data-repo-key', proj.repo_key);

      var title = proj.canonical_path || ('unknown path (' + proj.repo_key + ')');
      btn.appendChild(el('span', 'all-projects-item-path', title));

      var meta = el('span', 'all-projects-item-meta');
      meta.appendChild(el('span', 'all-projects-count',
        String(proj.conversation_count || 0) + ' conversations \u00b7 ' +
        String(proj.run_count || 0) + ' runs'));
      if (proj.active_run_count > 0) {
        meta.appendChild(el('span', 'badge all-projects-active-badge',
          String(proj.active_run_count) + ' active'));
      }
      if (proj.missing) {
        meta.appendChild(el('span', 'badge all-projects-missing-badge', 'missing'));
      }
      if (proj.open_ws) {
        meta.appendChild(el('span', 'badge all-projects-open-badge', 'open'));
      }
      btn.appendChild(meta);
      btn.addEventListener('click', function () {
        selectProject(proj.repo_key);
      });
      li.appendChild(btn);
      list.appendChild(li);
    });
  }

  function selectProject(repoKey) {
    closeStream();
    selectedRepoKey = repoKey;
    selectedConvId = null;
    selectedRunId = null;
    renderProjectsList();

    var detail = $('all-projects-detail');
    var itemPane = $('all-projects-item-detail');
    if (itemPane) { itemPane.hidden = true; itemPane.replaceChildren(); }
    if (detail) { detail.hidden = false; }

    var proj = selectedProject();
    var titleEl = $('all-projects-detail-title');
    if (titleEl && proj) {
      titleEl.textContent = proj.canonical_path || ('unknown path (' + proj.repo_key + ')');
    }
    updateOpenButton();

    var convList = $('all-projects-conversations');
    var runList = $('all-projects-runs');
    if (convList) { convList.replaceChildren(); }
    if (runList) { runList.replaceChildren(); }

    var encKey = encodeURIComponent(repoKey);
    Promise.all([
      fetchJson(apiUrl('/api/global/projects/' + encKey + '/conversations')).catch(function () { return { conversations: [] }; }),
      fetchJson(apiUrl('/api/global/projects/' + encKey + '/runs')).catch(function () { return { runs: [] }; })
    ]).then(function (results) {
      if (selectedRepoKey !== repoKey) { return; }
      renderConversations(repoKey, (results[0] && results[0].conversations) || []);
      renderRuns(repoKey, (results[1] && results[1].runs) || []);
    });
  }

  function renderConversations(repoKey, convs) {
    var list = $('all-projects-conversations');
    if (!list) { return; }
    list.replaceChildren();
    if (!convs.length) {
      list.appendChild(el('li', 'all-projects-sub-empty', 'No conversations'));
      return;
    }
    convs.forEach(function (c) {
      var li = el('li', 'all-projects-sub-item');
      var btn = el('button', 'btn btn-small all-projects-sub-btn');
      btn.type = 'button';
      btn.setAttribute('data-conv-id', c.id);
      btn.appendChild(el('span', 'all-projects-sub-title', c.title || c.id));
      btn.appendChild(el('span', 'all-projects-sub-meta', String(c.turn_count || 0) + ' turns'));
      btn.addEventListener('click', function () {
        loadConversationDetail(repoKey, c.id);
      });
      li.appendChild(btn);
      list.appendChild(li);
    });
  }

  function renderRuns(repoKey, runs) {
    var list = $('all-projects-runs');
    if (!list) { return; }
    list.replaceChildren();
    if (!runs.length) {
      list.appendChild(el('li', 'all-projects-sub-empty', 'No runs'));
      return;
    }
    runs.forEach(function (r) {
      var li = el('li', 'all-projects-sub-item');
      var btn = el('button', 'btn btn-small all-projects-sub-btn');
      btn.type = 'button';
      btn.setAttribute('data-run-id', r.run_id);
      var runner = r.resolved_runner || r.requested_runner || '';
      var label = (r.status || 'unknown') + (runner ? ' \u00b7 ' + runner : '');
      btn.appendChild(el('span', 'all-projects-sub-title', r.run_id));
      btn.appendChild(el('span', 'all-projects-sub-meta', label));
      btn.addEventListener('click', function () {
        loadRunDetail(repoKey, r.run_id);
      });
      li.appendChild(btn);
      list.appendChild(li);
    });
  }

  function loadConversationDetail(repoKey, convId) {
    closeStream();
    selectedConvId = convId;
    selectedRunId = null;
    var pane = $('all-projects-item-detail');
    if (!pane) { return; }
    pane.hidden = false;
    pane.replaceChildren(el('p', 'hint', 'Loading conversation\u2026'));

    var url = apiUrl('/api/global/projects/' + encodeURIComponent(repoKey) +
      '/conversations/' + encodeURIComponent(convId));
    fetchJson(url).then(function (data) {
      if (selectedRepoKey !== repoKey || selectedConvId !== convId) { return; }
      var conv = data && data.conversation;
      pane.replaceChildren();
      var head = el('div', 'all-projects-readonly-head');
      head.appendChild(el('h4', 'all-projects-readonly-title', (conv && conv.title) || convId));
      head.appendChild(el('span', 'badge all-projects-readonly-badge', 'read-only'));
      pane.appendChild(head);

      var msgs = (conv && conv.messages) || [];
      if (!msgs.length) {
        pane.appendChild(el('p', 'empty', 'No messages yet.'));
        return;
      }
      var ol = el('ol', 'all-projects-transcript');
      msgs.forEach(function (m) {
        var li = el('li', 'all-projects-msg all-projects-msg-' + (m.role || 'user'));
        var who = (m.role === 'assistant' && m.agent) ? m.agent : (m.role || 'message');
        li.appendChild(el('div', 'all-projects-msg-role', who));
        li.appendChild(el('div', 'all-projects-msg-text', m.text || ''));
        ol.appendChild(li);
      });
      pane.appendChild(ol);
    }).catch(function () {
      if (selectedRepoKey !== repoKey || selectedConvId !== convId) { return; }
      pane.replaceChildren(el('p', 'empty', 'Failed to load conversation.'));
    });
  }

  function loadRunDetail(repoKey, runId) {
    closeStream();
    selectedConvId = null;
    selectedRunId = runId;
    var pane = $('all-projects-item-detail');
    if (!pane) { return; }
    pane.hidden = false;
    pane.replaceChildren(el('p', 'hint', 'Loading run\u2026'));

    var url = apiUrl('/api/global/projects/' + encodeURIComponent(repoKey) +
      '/runs/' + encodeURIComponent(runId));
    fetchJson(url).then(function (data) {
      if (selectedRepoKey !== repoKey || selectedRunId !== runId) { return; }
      var run = (data && data.run) || {};
      pane.replaceChildren();
      var head = el('div', 'all-projects-readonly-head');
      head.appendChild(el('h4', 'all-projects-readonly-title', 'Run ' + (run.run_id || runId)));
      head.appendChild(el('span', 'badge all-projects-readonly-badge', 'read-only'));
      pane.appendChild(head);

      var statusEl = el('div', 'all-projects-run-status',
        'Status: ' + (run.status || 'unknown') +
        (run.resolved_runner || run.requested_runner ? ' \u00b7 ' + (run.resolved_runner || run.requested_runner) : ''));
      pane.appendChild(statusEl);

      var logOl = el('ol', 'all-projects-run-log');
      var progress = (data && data.progress) || [];
      progress.forEach(function (entry) {
        logOl.appendChild(el('li', 'all-projects-run-log-line',
          typeof entry === 'string' ? entry : JSON.stringify(entry)));
      });
      pane.appendChild(logOl);

      if (run.status === 'active' || run.status === 'running') {
        var streamUrl = '/api/global/projects/' + encodeURIComponent(repoKey) +
          '/runs/' + encodeURIComponent(runId) + '/stream';
        var tok = getToken();
        if (tok) {
          streamUrl += '?token=' + encodeURIComponent(tok);
        }
        try {
          var es = new EventSource(streamUrl);
          activeStream = es;
          es.addEventListener('run', function (ev) {
            try {
              var snap = JSON.parse(ev.data);
              if (snap && snap.status) {
                statusEl.textContent = 'Status: ' + snap.status +
                  (snap.resolved_runner || snap.requested_runner ? ' \u00b7 ' + (snap.resolved_runner || snap.requested_runner) : '');
              }
            } catch (err) { /* ignore */ }
          });
          es.addEventListener('progress', function (ev) {
            if (ev && ev.data) {
              logOl.appendChild(el('li', 'all-projects-run-log-line', ev.data));
            }
          });
          es.addEventListener('end', function () {
            closeStream();
          });
        } catch (err) { /* ignore */ }
      }
    }).catch(function () {
      if (selectedRepoKey !== repoKey || selectedRunId !== runId) { return; }
      pane.replaceChildren(el('p', 'empty', 'Failed to load run.'));
    });
  }

  function openSelectedProject() {
    var proj = selectedProject();
    if (!proj || proj.missing || !proj.canonical_path || viewOnlyMode || !canOpenInWeb) {
      return;
    }
    var btn = $('all-projects-open-btn');
    if (btn) { btn.disabled = true; }
    postJson(apiUrl('/api/workspace/attach'), { path: proj.canonical_path }).then(function (res) {
      if (!res.ok || !res.body || !res.body.id) {
        updateOpenButton();
        return;
      }
      var wsId = res.body.id;
      var p = Promise.resolve();
      if (selectedConvId) {
        p = postJson(apiUrlFor(wsId, '/api/command'), {
          command: 'load_conversation',
          id: selectedConvId
        });
      }
      return p.then(function () {
        var nextUrl = new URL(window.location.href);
        if (wsId === 'default') {
          nextUrl.searchParams.delete('ws');
        } else {
          nextUrl.searchParams.set('ws', wsId);
        }
        window.location.assign(nextUrl.toString());
      });
    }).catch(function () {
      updateOpenButton();
    });
  }

  function refresh() {
    Promise.all([
      fetchJson(apiUrl('/api/global/projects')).catch(function () { return { projects: [] }; }),
      fetchJson(apiUrl('/api/workspace')).catch(function () { return {}; })
    ]).then(function (results) {
      var projData = results[0] || {};
      var wsData = results[1] || {};
      projects = Array.isArray(projData.projects) ? projData.projects : [];
      viewOnlyMode = wsData.view_only !== false && !wsData.can_switch;
      canOpenInWeb = !!wsData.can_switch && !wsData.view_only;
      renderProjectsList();
      updateOpenButton();
    });
  }

  function init() {
    var openBtn = $('all-projects-open-btn');
    if (openBtn) {
      openBtn.addEventListener('click', openSelectedProject);
    }
    document.addEventListener('dmuxai:center-view', function (ev) {
      var isNowOpen = !!(ev && ev.detail && ev.detail.view === 'all-projects');
      if (isNowOpen && !open) {
        open = true;
        refresh();
      } else if (!isNowOpen && open) {
        open = false;
        closeStream();
      }
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
