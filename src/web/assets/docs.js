'use strict';

// ---------------------------------------------------------------------------
// Document preview and editing (SPEC-docs-preview FR-746..FR-749, UX-753,
// INT-754). Isolated module: the to-do/graph panel code never imports this;
// it only dispatches 'dmuxai:snapshot' CustomEvents that we listen for.
// ---------------------------------------------------------------------------

(function () {
  var TOKEN_KEY = 'dmuxai.token';
  var SNAPSHOT_EVENT = 'dmuxai:snapshot';
  var CHANGES_POLL_MS = 10000;
  // Snapshots can arrive every second (app.js tick); FR-748 changes only move
  // on turn boundaries, so snapshot-triggered refreshes are coalesced to at
  // most one per interval.
  var SNAPSHOT_REFRESH_MIN_MS = 2000;
  var RENDER_DEBOUNCE_MS = 250;

  var KIND_ICON = {
    markdown: 'MD',
    text: 'TXT',
    html: 'HTML',
    pdf: 'PDF',
    office: 'DOC',
    binary: 'BIN'
  };

  // The token rotates on every server start and app.js can drop it on 401,
  // so it is re-read from sessionStorage for every request instead of being
  // captured once at module load.
  function currentToken() {
    try {
      return window.sessionStorage.getItem(TOKEN_KEY);
    } catch (err) {
      return null;
    }
  }

  // False until /api/state (or a snapshot) tells us whether this page is
  // `dmuxai --web` or read-only `dmuxai serve`; Edit stays disabled until
  // then so a serve user cannot even attempt a PUT.
  var readOnly = false;
  var readOnlyKnown = false;
  var entries = [];
  var entriesLoaded = false;
  var entriesError = '';
  var listTruncated = false;
  var filterText = '';
  var changesTimer = null;
  var snapshotRefreshTimer = null;
  var lastSnapshotRefreshMs = 0;

  var pane = null;
  var docBody = null;
  var docBodies = {};
  // current: { path, kind, editable, sha256, text, html, download,
  //            editing, dirty } while a document is open; null when closed.
  var current = null;
  var conflictPayload = null;
  var renderTimer = null;

  function $(id) {
    return document.getElementById(id);
  }

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) { node.className = className; }
    if (text !== undefined && text !== null) { node.textContent = text; }
    return node;
  }

  // -----------------------------------------------------------------------
  // SPEC-dashboard-fullwidth FR-1202..FR-1204: the center Documents panel is
  // collapsed by default; the toggle state lives in sessionStorage only.
  // -----------------------------------------------------------------------

  var DOCS_EXPANDED_KEY = 'dmuxai.docsExpanded';

  function readDocsExpanded() {
    try { return window.sessionStorage.getItem(DOCS_EXPANDED_KEY) === '1'; } catch (err) { return false; }
  }

  function writeDocsExpanded(expanded) {
    try { window.sessionStorage.setItem(DOCS_EXPANDED_KEY, expanded ? '1' : '0'); } catch (err) { /* ignore */ }
  }

  function setDocsExpanded(expanded) {
    var toggle = $('docs-toggle');
    var body = $('docs-body');
    var panel = $('docs');
    if (!toggle || !body) { return; }
    toggle.setAttribute('aria-expanded', expanded ? 'true' : 'false');
    body.hidden = !expanded;
    if (panel) { panel.setAttribute('data-collapsed', expanded ? 'false' : 'true'); }
  }

  function initDocsToggle() {
    var toggle = $('docs-toggle');
    if (!toggle) { return; }
    var panel = $('docs');
    // An open Diff view (restored by app.js) always shows the list.
    setDocsExpanded(readDocsExpanded() || !!(panel && !panel.hidden));
    // A native <button> already handles Enter and Space.
    toggle.addEventListener('click', function () {
      var expanded = toggle.getAttribute('aria-expanded') !== 'true';
      setDocsExpanded(expanded);
      writeDocsExpanded(expanded);
    });
  }

  function setDocsCount(n) {
    // FR-1212: the header Diff button carries the same count.
    var diffCount = $('diff-count');
    if (diffCount) {
      diffCount.textContent = String(n || 0);
      diffCount.hidden = !(typeof n === 'number' && n > 0);
      var diffToggle = $('diff-toggle');
      if (diffToggle) {
        diffToggle.setAttribute('aria-label', n > 0 ? 'Diff, ' + n + ' changed' : 'Diff');
      }
    }
    var count = $('docs-count');
    if (!count) { return; }
    if (typeof n === 'number' && n > 0) {
      count.textContent = String(n);
      count.title = n + (n === 1 ? ' agent turn changed files' : ' agent turns changed files');
      count.hidden = false;
    } else {
      count.textContent = '';
      count.hidden = true;
    }
  }

  function docBadge(label, className, title) {
    var node = el('span', 'badge' + (className ? ' ' + className : ''), label);
    if (title) { node.title = title; }
    return node;
  }

  function kindBadge(kind) {
    var key = String(kind || 'binary');
    return docBadge(
      KIND_ICON[key] || 'FILE',
      'doc-kind doc-kind-' + key,
      'kind: ' + key
    );
  }

  function fmtTime(ms) {
    var d = new Date(ms);
    if (isNaN(d.getTime())) { return ''; }
    return d.toLocaleTimeString([], {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit'
    });
  }

  function truncate(text, max) {
    if (typeof text !== 'string') { return ''; }
    if (text.length <= max) { return text; }
    return text.slice(0, max) + '\u2026';
  }

  // -----------------------------------------------------------------------
  // Authenticated docs API (dashboard token, NFR-751).
  // -----------------------------------------------------------------------

  function api(method, path, body) {
    // Re-read per request: the token rotates and app.js removes it on 401.
    var token = currentToken();
    if (!token) {
      return Promise.reject(
        fatalError('missing dashboard token (reopen the ?token= URL)')
      );
    }
    var headers = { Authorization: 'Bearer ' + token };
    var init = { method: method, headers: headers, cache: 'no-store' };
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(body);
    }
    return fetch(path, init).then(function (res) {
      if (res.status === 401) {
        // Token rotation rejected this page; stop polling instead of
        // hammering the server with stale credentials.
        stopChangesPolling();
        return Promise.reject(
          fatalError('unauthorized: reopen the dashboard URL (?token=...)')
        );
      }
      return res.text().then(function (text) {
        var payload = null;
        if (text) {
          try {
            payload = JSON.parse(text);
          } catch (err) {
            payload = null;
          }
        }
        if (!res.ok) {
          var failure = new Error(
            payload && typeof payload.error === 'string'
              ? payload.error
              : 'HTTP ' + res.status
          );
          failure.status = res.status;
          failure.payload = payload;
          throw failure;
        }
        return payload;
      });
    });
  }

  function fatalError(text) {
    var failure = new Error(text);
    failure.fatal = true;
    return failure;
  }

  // -----------------------------------------------------------------------
  // Sidebar shell and collapsible files tree (SPEC-docs-tree FR-832, FR-833, UX-830).
  // -----------------------------------------------------------------------

  var treeDirCache = {};
  var expandedFolders = [];
  var currentWorkspace = null;
  var treeIdSeq = 0;
  var pendingOpen = null;
  // Active sidebar tab: 'files' or 'todos' (SPEC-todo-sidebar FR-842).
  var SIDEBAR_TABS = ['files', 'todos'];
  var SIDEBAR_TOGGLES = { files: 'sidebar-toggle-files', todos: 'todos-toggle' };
  var activeTab = 'files';
  var pendingTab = null;
  var pendingExpanded = null;
  var pendingFocusPath = null;

  function applyWorkspaceSnapshot(snap) {
    if (!snap) { return; }
    var ws = typeof snap.workspace === 'string' ? snap.workspace.trim() : '';
    if (!ws || ws === '-' || ws === '\u2013') {
      var wsEl = $('workspace');
      if (wsEl && wsEl.textContent && wsEl.textContent !== '\u2013' && wsEl.textContent !== '-') {
        ws = wsEl.textContent.trim();
      }
    }
    if (ws && ws !== '-' && ws !== '\u2013' && ws !== currentWorkspace) {
      currentWorkspace = ws;
      treeDirCache = {};
      pendingFocusPath = null;
      var state = loadSidebarState();
      if (pendingOpen !== null) {
        state.open = pendingOpen;
        saveSidebarOpen(pendingOpen);
        pendingOpen = null;
      }
      if (pendingTab !== null) {
        state.tab = pendingTab;
        saveSidebarTab(pendingTab);
        pendingTab = null;
      }
      setActiveTab(state.tab, true);
      if (pendingExpanded !== null) {
        state.expanded = pendingExpanded;
        saveExpandedFolders(pendingExpanded);
        pendingExpanded = null;
      }
      expandedFolders = state.expanded;
      updateSidebarUI(state.open);
      if (!filterText) {
        renderLazyTree();
      }
    }
  }

  function loadSidebarState() {
    try {
      if (!currentWorkspace) {
        return { open: false, expanded: [] };
      }
      var ws = currentWorkspace;
      var openVal = window.localStorage.getItem('dmuxai.sidebar.open.' + ws);
      var expVal = window.localStorage.getItem('dmuxai.sidebar.expanded.' + ws);
      var tabVal = window.localStorage.getItem('dmuxai.sidebar.tab.' + ws);
      var open = openVal === 'true';
      var exp = expVal ? JSON.parse(expVal) : [];
      return {
        open: open,
        expanded: Array.isArray(exp) ? exp : [],
        tab: SIDEBAR_TABS.indexOf(tabVal) !== -1 ? tabVal : 'files'
      };
    } catch (e) {
      return { open: false, expanded: [], tab: 'files' };
    }
  }

  function saveSidebarTab(tab) {
    if (!currentWorkspace) {
      pendingTab = tab;
      return;
    }
    try {
      window.localStorage.setItem('dmuxai.sidebar.tab.' + currentWorkspace, tab);
    } catch (e) {}
  }

  // Switches the visible tab panel; the toggle buttons follow in updateSidebarUI.
  function setActiveTab(tab, skipSave) {
    if (SIDEBAR_TABS.indexOf(tab) === -1) { tab = 'files'; }
    activeTab = tab;
    for (var i = 0; i < SIDEBAR_TABS.length; i++) {
      var name = SIDEBAR_TABS[i];
      var tabBtn = $('tab-' + name);
      var panel = $('tab-panel-' + name);
      var on = name === tab;
      if (tabBtn) {
        tabBtn.classList.toggle('active', on);
        tabBtn.setAttribute('aria-selected', on ? 'true' : 'false');
        tabBtn.setAttribute('tabindex', on ? '0' : '-1');
      }
      if (panel) { panel.hidden = !on; }
    }
    if (!skipSave) { saveSidebarTab(tab); }
    var aside = $('sidebar');
    updateSidebarUI(Boolean(aside && aside.classList.contains('open')));
  }

  function saveSidebarOpen(open) {
    if (!currentWorkspace) {
      pendingOpen = open;
      return;
    }
    try {
      window.localStorage.setItem('dmuxai.sidebar.open.' + currentWorkspace, open ? 'true' : 'false');
    } catch (e) {}
  }

  function saveExpandedFolders(list) {
    if (!currentWorkspace) {
      pendingExpanded = list;
      return;
    }
    try {
      window.localStorage.setItem('dmuxai.sidebar.expanded.' + currentWorkspace, JSON.stringify(list));
    } catch (e) {}
  }

  function updateSidebarUI(open) {
    var aside = $('sidebar');
    var backdrop = $('sidebar-backdrop');
    if (aside) {
      if (open) {
        aside.classList.add('open');
        aside.removeAttribute('inert');
        document.body.classList.add('sidebar-open');
      } else {
        aside.classList.remove('open');
        aside.setAttribute('inert', '');
        document.body.classList.remove('sidebar-open');
      }
    }
    // Each toggle is "expanded" only while the sidebar shows its tab.
    for (var t = 0; t < SIDEBAR_TABS.length; t++) {
      var btn = $(SIDEBAR_TOGGLES[SIDEBAR_TABS[t]]);
      if (!btn) { continue; }
      var shown = open && activeTab === SIDEBAR_TABS[t];
      btn.setAttribute('aria-expanded', shown ? 'true' : 'false');
      btn.classList.toggle('active', shown);
    }
    if (backdrop) {
      if (open) {
        backdrop.classList.add('active');
      } else {
        backdrop.classList.remove('active');
      }
    }
  }

  function openSidebar() {
    saveSidebarOpen(true);
    updateSidebarUI(true);
    var isNarrow = (window.innerWidth <= 900) || (window.matchMedia && window.matchMedia('(max-width: 900px)').matches);
    if (isNarrow) {
      var focusTarget = (activeTab === 'todos' ? $('todo-search') : $('docs-filter')) || $('sidebar-close');
      if (focusTarget && typeof focusTarget.focus === 'function') {
        focusTarget.focus();
      }
    }
  }

  function closeSidebar() {
    saveSidebarOpen(false);
    updateSidebarUI(false);
    var toggleBtn = $(SIDEBAR_TOGGLES[activeTab]);
    if (toggleBtn && typeof toggleBtn.focus === 'function') {
      toggleBtn.focus();
    }
  }

  // A header toggle closes the sidebar when it already shows that tab and
  // otherwise opens it on that tab (FR-842).
  function toggleSidebarTab(tab) {
    var aside = $('sidebar');
    var isOpen = aside ? aside.classList.contains('open') : false;
    if (isOpen && activeTab === tab) {
      closeSidebar();
    } else {
      setActiveTab(tab);
      openSidebar();
    }
  }

  // Esc closes the sidebar unless focus is in a text field or the docs
  // editor, or a modal (the preview) is open: those handle Esc themselves.
  function escapeClosesSidebar(e) {
    if (e.defaultPrevented) { return false; }
    var target = e.target;
    if (target && target.nodeType === 1) {
      var tag = target.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || target.isContentEditable) {
        return false;
      }
    }
    var modals = document.querySelectorAll('[aria-modal="true"]');
    for (var i = 0; i < modals.length; i++) {
      if (!modals[i].hidden && modals[i].getClientRects().length > 0) { return false; }
    }
    return true;
  }

  function fetchDir(dir) {
    if (treeDirCache[dir]) {
      return Promise.resolve(treeDirCache[dir]);
    }
    var url = '/api/docs/tree?dir=' + encodeURIComponent(dir);
    return api('GET', url).then(function (payload) {
      var dirEntries = payload && Array.isArray(payload.entries) ? payload.entries : [];
      treeDirCache[dir] = dirEntries;
      return dirEntries;
    });
  }

  function entriesSignature(list) {
    if (!Array.isArray(list)) { return ''; }
    var s = [];
    for (var i = 0; i < list.length; i++) {
      var e = list[i] || {};
      s.push((e.path || '') + ':' + (e.mtime_ms || 0) + ':' + (e.has_children ? '1' : '0'));
    }
    return s.join(';');
  }

  function getFocusedTreePath() {
    var active = document.activeElement;
    if (active && active.classList.contains('tree-item') && active.dataset.path) {
      return active.dataset.path;
    }
    return null;
  }

  function restoreFocusToPath(path) {
    if (!path) { return false; }
    var items = getVisibleTreeItems();
    for (var i = 0; i < items.length; i++) {
      if (items[i].dataset.path === path) {
        setFocusTreeItem(items[i]);
        if (pendingFocusPath === path) { pendingFocusPath = null; }
        return true;
      }
    }
    // The item may live in a folder whose children are still loading; retry
    // when that group finishes rendering (see toggleFolder).
    pendingFocusPath = path;
    return false;
  }

  function createDirItem(entry, level) {
    var node = el('div', 'tree-node');
    node.setAttribute('role', 'none');

    var item = el('div', 'tree-item');
    item.setAttribute('role', 'treeitem');
    item.setAttribute('aria-expanded', 'false');
    item.setAttribute('tabindex', '-1');
    item.setAttribute('aria-level', String(level));
    item.dataset.path = entry.path;
    item.dataset.type = 'dir';

    var groupId = 'tree-group-' + (++treeIdSeq);
    item.id = 'tree-item-' + treeIdSeq;

    var toggle = el('span', 'tree-toggle', '\u25b6');
    var icon = el('span', 'tree-icon', '\ud83d\udcc1');
    var name = el('span', 'tree-name', entry.name || entry.path);

    item.appendChild(toggle);
    item.appendChild(icon);
    item.appendChild(name);
    node.appendChild(item);

    var group = el('div', 'tree-group');
    group.id = groupId;
    group.setAttribute('role', 'group');
    group.setAttribute('hidden', '');
    item.setAttribute('aria-owns', group.id);
    node.appendChild(group);

    item.addEventListener('click', function (e) {
      e.stopPropagation();
      setFocusTreeItem(item);
      toggleFolder(item);
    });

    if (expandedFolders.indexOf(entry.path) !== -1) {
      toggleFolder(item, true);
    }

    return node;
  }

  function createFileItem(entry, level) {
    var node = el('div', 'tree-node');
    node.setAttribute('role', 'none');

    var item = el('div', 'tree-item');
    item.setAttribute('role', 'treeitem');
    item.setAttribute('tabindex', '-1');
    item.setAttribute('aria-level', String(level));
    item.dataset.path = entry.path;
    item.dataset.type = 'file';

    var placeholder = el('span', 'tree-toggle');
    var badge = kindBadge(entry.kind);
    var name = el('span', 'tree-name', entry.name || entry.path);

    item.appendChild(placeholder);
    item.appendChild(badge);
    item.appendChild(name);
    node.appendChild(item);

    item.addEventListener('click', function (e) {
      e.stopPropagation();
      setFocusTreeItem(item);
      open(entry.path);
    });

    return node;
  }

  function toggleFolder(item, forceExpand) {
    var path = item.dataset.path;
    var isExpanded = item.getAttribute('aria-expanded') === 'true';
    var shouldExpand = typeof forceExpand === 'boolean' ? forceExpand : !isExpanded;
    var node = item.closest('.tree-node');
    var group = node ? node.querySelector(':scope > .tree-group') : null;
    var toggle = item.querySelector('.tree-toggle');

    if (shouldExpand) {
      item.setAttribute('aria-expanded', 'true');
      if (toggle) {
        toggle.classList.add('expanded');
        toggle.textContent = '\u25bc';
      }
      if (expandedFolders.indexOf(path) === -1) {
        expandedFolders.push(path);
        saveExpandedFolders(expandedFolders);
      }
      if (group) {
        group.removeAttribute('hidden');
        if (!group.hasChildNodes() || group._failed) {
          group._failed = false;
          group.replaceChildren();
          group.appendChild(el('div', 'tree-hint', 'loading\u2026'));
          fetchDir(path).then(function (children) {
            var groupSig = entriesSignature(children);
            if (group._lastSig === groupSig && group.hasChildNodes()) {
              return;
            }
            group._lastSig = groupSig;
            // Only restore a pending focus while focus is still on the page
            // body or in the tree, never stealing it from the filter or preview.
            var active = document.activeElement;
            var treeEl = $('docs-tree');
            var mayRestore = !active || active === document.body ||
              (treeEl && treeEl.contains(active));
            var focusedPath = getFocusedTreePath() || (mayRestore ? pendingFocusPath : null);
            group.replaceChildren();
            renderChildrenInto(group, children, parseInt(item.getAttribute('aria-level') || '1', 10) + 1);
            updateRovingTabindex();
            if (focusedPath) {
              restoreFocusToPath(focusedPath);
            }
          }).catch(function () {
            group.replaceChildren();
            group._failed = true; // the next expand retries the fetch
            group.appendChild(el('div', 'tree-hint', 'error loading'));
          });
        }
      }
    } else {
      item.setAttribute('aria-expanded', 'false');
      if (toggle) {
        toggle.classList.remove('expanded');
        toggle.textContent = '\u25b6';
      }
      var idx = expandedFolders.indexOf(path);
      if (idx !== -1) {
        expandedFolders.splice(idx, 1);
        saveExpandedFolders(expandedFolders);
      }
      if (group) {
        group.setAttribute('hidden', '');
      }
    }
    updateRovingTabindex();
  }

  function renderChildrenInto(container, childEntries, level) {
    for (var i = 0; i < childEntries.length; i++) {
      var entry = childEntries[i];
      if (entry.entry_type === 'dir' || entry.type === 'dir') {
        container.appendChild(createDirItem(entry, level));
      } else {
        container.appendChild(createFileItem(entry, level));
      }
    }
  }

  function renderLazyTree(silent) {
    var tree = $('docs-tree');
    if (!tree) { return; }
    if (!currentToken()) {
      tree.replaceChildren();
      tree.appendChild(el('p', 'empty', 'documents need token'));
      return;
    }
    var focusedPath = getFocusedTreePath();
    if (!silent && !tree.hasChildNodes()) {
      tree.replaceChildren();
      tree.appendChild(el('div', 'tree-hint', 'loading\u2026'));
    }

    return fetchDir('').then(function (children) {
      var sig = entriesSignature(children);
      if (tree._lastSig === sig && tree.hasChildNodes()) {
        if (focusedPath) {
          restoreFocusToPath(focusedPath);
        }
        return;
      }
      tree._lastSig = sig;
      tree.replaceChildren();
      if (children.length === 0) {
        tree.appendChild(el('p', 'empty', 'no documents'));
        return;
      }
      renderChildrenInto(tree, children, 1);
      updateRovingTabindex();
      if (focusedPath) {
        restoreFocusToPath(focusedPath);
      }
    }).catch(function (err) {
      if (!silent) {
        tree.replaceChildren();
        tree.appendChild(el('p', 'empty', 'tree unavailable: ' + err.message));
      }
    });
  }

  function renderFilteredTree(needle) {
    var tree = $('docs-tree');
    if (!tree) { return; }
    var focusedPath = getFocusedTreePath();
    tree.replaceChildren();
    if (!entriesLoaded) {
      tree.appendChild(entriesError
        ? el('p', 'empty', 'documents unavailable: ' + entriesError)
        : el('div', 'tree-hint', 'loading\u2026'));
      return;
    }
    if (listTruncated) {
      // The filter searches the capped /api/docs list; say so instead of
      // silently missing files past the cap.
      tree.appendChild(el('p', 'tree-hint', 'file list truncated; some files may not match'));
    }

    var matches = [];
    for (var i = 0; i < entries.length; i++) {
      var entry = entries[i] || {};
      var p = String(entry.path || '');
      if (p.toLowerCase().indexOf(needle) !== -1) {
        matches.push(entry);
      }
    }

    if (matches.length === 0) {
      tree.appendChild(el('p', 'empty', 'no documents match the filter'));
      return;
    }

    // Build hierarchical tree structure from matching paths
    var rootObj = { dirs: {}, files: [] };
    for (var j = 0; j < matches.length; j++) {
      var match = matches[j];
      var parts = match.path.split('/');
      var curr = rootObj;
      var curPath = '';
      for (var k = 0; k < parts.length - 1; k++) {
        var dirName = parts[k];
        curPath = curPath ? curPath + '/' + dirName : dirName;
        if (!curr.dirs[dirName]) {
          curr.dirs[dirName] = { name: dirName, path: curPath, dirs: {}, files: [] };
        }
        curr = curr.dirs[dirName];
      }
      curr.files.push(match);
    }

    function renderFilterNode(container, obj, level) {
      var dirKeys = Object.keys(obj.dirs);
      dirKeys.sort();
      for (var d = 0; d < dirKeys.length; d++) {
        var dirData = obj.dirs[dirKeys[d]];
        var node = el('div', 'tree-node');
        node.setAttribute('role', 'none');

        var item = el('div', 'tree-item');
        item.setAttribute('role', 'treeitem');
        item.setAttribute('aria-expanded', 'true');
        item.setAttribute('tabindex', '-1');
        item.setAttribute('aria-level', String(level));
        item.dataset.path = dirData.path;
        item.dataset.type = 'dir';

        var groupId = 'tree-group-' + (++treeIdSeq);
        item.id = 'tree-item-' + treeIdSeq;

        var toggle = el('span', 'tree-toggle expanded', '\u25bc');
        var icon = el('span', 'tree-icon', '\ud83d\udcc1');
        var name = el('span', 'tree-name', dirData.name);

        item.appendChild(toggle);
        item.appendChild(icon);
        item.appendChild(name);
        node.appendChild(item);

        var group = el('div', 'tree-group');
        group.id = groupId;
        group.setAttribute('role', 'group');
        item.setAttribute('aria-owns', group.id);
        node.appendChild(group);

        item.addEventListener('click', function (e) {
          e.stopPropagation();
          setFocusTreeItem(item);
          toggleFolder(item);
        });

        renderFilterNode(group, dirData, level + 1);
        container.appendChild(node);
      }

      obj.files.sort(function (a, b) {
        var na = a.path.split('/').pop();
        var nb = b.path.split('/').pop();
        return na.localeCompare(nb);
      });
      for (var f = 0; f < obj.files.length; f++) {
        var fileEntry = obj.files[f];
        var fileNode = createFileItem({
          name: fileEntry.path.split('/').pop(),
          path: fileEntry.path,
          kind: fileEntry.kind
        }, level);
        container.appendChild(fileNode);
      }
    }

    renderFilterNode(tree, rootObj, 1);
    updateRovingTabindex();
    if (focusedPath) {
      restoreFocusToPath(focusedPath);
    }
  }

  function renderTree() {
    var needle = filterText.toLowerCase().trim();
    if (needle) {
      renderFilteredTree(needle);
    } else {
      renderLazyTree();
    }
  }

  function getVisibleTreeItems() {
    var tree = $('docs-tree');
    if (!tree) { return []; }
    var allItems = tree.querySelectorAll('.tree-item');
    var visible = [];
    for (var i = 0; i < allItems.length; i++) {
      var item = allItems[i];
      var group = item.closest('.tree-group[hidden]');
      if (!group) {
        visible.push(item);
      }
    }
    return visible;
  }

  function updateRovingTabindex() {
    var visibleItems = getVisibleTreeItems();
    var hasFocusable = false;
    for (var i = 0; i < visibleItems.length; i++) {
      if (visibleItems[i].getAttribute('tabindex') === '0') {
        hasFocusable = true;
        break;
      }
    }
    if (!hasFocusable && visibleItems.length > 0) {
      visibleItems[0].setAttribute('tabindex', '0');
    }
  }

  function setFocusTreeItem(target) {
    var items = getVisibleTreeItems();
    for (var i = 0; i < items.length; i++) {
      items[i].setAttribute('tabindex', '-1');
    }
    target.setAttribute('tabindex', '0');
    target.focus();
  }

  function focusNextTreeItem(current) {
    var items = getVisibleTreeItems();
    var idx = items.indexOf(current);
    if (idx !== -1 && idx + 1 < items.length) {
      setFocusTreeItem(items[idx + 1]);
    }
  }

  function focusPrevTreeItem(current) {
    var items = getVisibleTreeItems();
    var idx = items.indexOf(current);
    if (idx > 0) {
      setFocusTreeItem(items[idx - 1]);
    }
  }

  function bindTreeKeyboard() {
    var tree = $('docs-tree');
    if (!tree) { return; }

    tree.addEventListener('keydown', function (e) {
      var active = document.activeElement;
      if (!active || !active.classList.contains('tree-item')) {
        return;
      }
      var key = e.key;
      if (key === 'ArrowDown' || key === 'Down') {
        e.preventDefault();
        focusNextTreeItem(active);
      } else if (key === 'ArrowUp' || key === 'Up') {
        e.preventDefault();
        focusPrevTreeItem(active);
      } else if (key === 'ArrowRight' || key === 'Right') {
        e.preventDefault();
        if (active.dataset.type === 'dir') {
          var isExp = active.getAttribute('aria-expanded') === 'true';
          if (!isExp) {
            toggleFolder(active, true);
          } else {
            focusNextTreeItem(active);
          }
        }
      } else if (key === 'ArrowLeft' || key === 'Left') {
        e.preventDefault();
        if (active.dataset.type === 'dir' && active.getAttribute('aria-expanded') === 'true') {
          toggleFolder(active, false);
        } else {
          var parentGroup = active.closest('.tree-group');
          if (parentGroup) {
            var parentNode = parentGroup.closest('.tree-node');
            if (parentNode) {
              var parentItem = parentNode.querySelector(':scope > .tree-item');
              if (parentItem) {
                setFocusTreeItem(parentItem);
              }
            }
          }
        }
      } else if (key === 'Enter') {
        e.preventDefault();
        if (active.dataset.type === 'dir') {
          toggleFolder(active);
        } else if (active.dataset.path) {
          open(active.dataset.path);
        }
      }
    });
  }

  function initSidebar() {
    var state = loadSidebarState();
    expandedFolders = state.expanded;
    setActiveTab(state.tab, true);
    updateSidebarUI(state.open);

    SIDEBAR_TABS.forEach(function (tab) {
      var toggleBtn = $(SIDEBAR_TOGGLES[tab]);
      if (toggleBtn) {
        toggleBtn.addEventListener('click', function () { toggleSidebarTab(tab); });
      }
      var tabBtn = $('tab-' + tab);
      if (tabBtn) {
        tabBtn.addEventListener('click', function () { setActiveTab(tab); });
        tabBtn.addEventListener('keydown', function (e) {
          if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') { return; }
          e.preventDefault();
          var next = SIDEBAR_TABS[(SIDEBAR_TABS.indexOf(tab) + 1) % SIDEBAR_TABS.length];
          setActiveTab(next);
          var nextBtn = $('tab-' + next);
          if (nextBtn) { nextBtn.focus(); }
        });
      }
    });

    var closeBtn = $('sidebar-close');
    if (closeBtn) {
      closeBtn.addEventListener('click', closeSidebar);
    }

    var backdrop = $('sidebar-backdrop');
    if (backdrop) {
      backdrop.addEventListener('click', closeSidebar);
    }

    document.addEventListener('keydown', function (e) {
      if (e.key === 'Escape' && escapeClosesSidebar(e)) {
        var aside = $('sidebar');
        if (aside && aside.classList.contains('open')) {
          closeSidebar();
        }
      }
    });

    bindTreeKeyboard();
  }

  // -----------------------------------------------------------------------
  // Document list (FR-741 over GET /api/docs).
  // -----------------------------------------------------------------------

  function refreshList() {
    if (!currentToken()) {
      renderTree();
      return;
    }
    treeDirCache = {};
    api('GET', '/api/docs')
      .then(function (payload) {
        entries = payload && Array.isArray(payload.entries) ? payload.entries : [];
        entriesLoaded = true;
        entriesError = '';
        listTruncated = Boolean(payload && payload.truncated);
        renderTree();
      })
      .catch(function (err) {
        entriesError = (err && err.message) || 'request failed';
        renderTree();
      });
  }

  function bindFilter() {
    var input = $('docs-filter');
    if (!input) { return; }
    input.addEventListener('input', function () {
      filterText = input.value;
      renderTree();
    });
  }

  // Only same-origin ticket URLs from the docs API reach iframe/anchor sinks.
  function ticketUrl(url) {
    var value = String(url || '');
    return /^\/docs\/(view|download)\/[A-Za-z0-9-]+$/.test(value) ? value : 'about:blank';
  }

  // -----------------------------------------------------------------------
  // Turn changes (FR-748 over GET /api/docs/changes) + INT-754 module hook.
  // -----------------------------------------------------------------------

  function changes(nodeId) {
    var url = '/api/docs/changes';
    if (nodeId) {
      url += '?node=' + encodeURIComponent(String(nodeId));
    }
    return api('GET', url).then(function (payload) {
      return payload && Array.isArray(payload.turns) ? payload.turns : [];
    });
  }

  var lastChangesSig = null;

  function changesSignature(turns) {
    if (!Array.isArray(turns)) { return ''; }
    var parts = [];
    for (var i = 0; i < turns.length; i++) {
      var t = turns[i] || {};
      var fileSig = [];
      var files = Array.isArray(t.files) ? t.files : [];
      for (var j = 0; j < files.length; j++) {
        var f = files[j] || {};
        fileSig.push(String(f.path || '') + ':' + String(f.change || ''));
      }
      parts.push(fileSig.join(','));
    }
    return parts.join(';');
  }

  function refreshChanges() {
    changes(null)
      .then(function (turns) {
        var sig = changesSignature(turns);
        var changed = (lastChangesSig !== null && sig !== lastChangesSig);
        lastChangesSig = sig;
        // The panel always re-renders so a running turn's output tail stays
        // fresh; the signature only decides whether the tree is invalidated.
        renderChanges(turns);
        if (changed) {
          treeDirCache = {};
          var tree = $('docs-tree');
          if (tree) { tree._lastSig = null; }
          if (!filterText) {
            renderLazyTree(true);
          } else {
            api('GET', '/api/docs').then(function (payload) {
              entries = payload && Array.isArray(payload.entries) ? payload.entries : [];
              entriesLoaded = true;
              listTruncated = Boolean(payload && payload.truncated);
              renderFilteredTree(filterText.toLowerCase().trim());
            }).catch(function () {});
          }
        }
      })
      .catch(function (err) {
        if (err && err.fatal) {
          // 401/missing token: the interval is already stopped by api();
          // surface it instead of silently retrying.
          var wrap = $('docs-changes');
          if (wrap) {
            wrap.replaceChildren();
            wrap.appendChild(el('p', 'empty', err.message));
          }
          return;
        }
        /* non-fatal: the poll retries silently */
      });
  }

  function startChangesPolling() {
    if (changesTimer !== null || !currentToken()) { return; }
    refreshChanges();
    changesTimer = window.setInterval(refreshChanges, CHANGES_POLL_MS);
  }

  function stopChangesPolling() {
    if (changesTimer !== null) {
      window.clearInterval(changesTimer);
      changesTimer = null;
    }
    if (snapshotRefreshTimer !== null) {
      window.clearTimeout(snapshotRefreshTimer);
      snapshotRefreshTimer = null;
    }
  }

  // Snapshot events arrive every second during a run; the change log only
  // moves on turn boundaries, so coalesce to at most one fetch per interval
  // (>= SNAPSHOT_REFRESH_MIN_MS apart).
  function scheduleSnapshotRefresh() {
    if (snapshotRefreshTimer !== null) { return; }
    var wait = lastSnapshotRefreshMs + SNAPSHOT_REFRESH_MIN_MS - Date.now();
    if (wait <= 0) {
      lastSnapshotRefreshMs = Date.now();
      refreshChanges();
      return;
    }
    snapshotRefreshTimer = window.setTimeout(function () {
      snapshotRefreshTimer = null;
      lastSnapshotRefreshMs = Date.now();
      refreshChanges();
    }, wait);
  }

  function changesRow(turn) {
    var li = el('li', 'doc-turn');
    li.id = docTurnDomId(turn.id);
    li.setAttribute('data-turn-id', String(turn.id || ''));
    if (turn.message_id) { li.setAttribute('data-msg-id', String(turn.message_id)); }
    li.dmuxTurn = turn;

    var head = el('div', 'doc-turn-head');
    head.appendChild(el('span', 'doc-agent', String(turn.agent || '?')));
    if (turn.node_id) {
      head.appendChild(el('span', 'dot', '\u00b7'));
      head.appendChild(el('span', 'doc-node', String(turn.node_id)));
    }
    head.appendChild(el('span', 'dot', '\u00b7'));
    var timeMs = typeof turn.finished_ms === 'number'
      ? turn.finished_ms
      : turn.started_ms;
    var time = el('span', 'doc-turn-time', fmtTime(timeMs));
    if (typeof timeMs === 'number') {
      time.title = new Date(timeMs).toLocaleString();
    }
    head.appendChild(time);
    li.appendChild(head);

    var files = Array.isArray(turn.files) ? turn.files : [];
    if (files.length > 0) {
      var wrap = el('div', 'doc-turn-files');
      for (var i = 0; i < files.length; i++) {
        var file = files[i] || {};
        var fileRow = el('span', 'doc-file');
        fileRow.appendChild(docLink(String(file.path || '')));
        var change = String(file.change || 'modified');
        fileRow.appendChild(
          docBadge(change, 'doc-chg doc-chg-' + change, 'file was ' + change)
        );
        wrap.appendChild(fileRow);
      }
      li.appendChild(wrap);
    }

    // The agent output goes in its own slot: syncChatLinks() fills it with
    // the full tail, or with a one-line reference when the chat already shows
    // that text (FR-1209).
    li.appendChild(el('div', 'doc-turn-out'));
    return li;
  }

  function turnTailText(turn) {
    // FR-748: output_tail is a JSON array of lines; join with real line
    // breaks so the tail reads as the agent's output.
    var tailLines = Array.isArray(turn.output_tail) ? turn.output_tail : [];
    return tailLines.length ? tailLines.join('\n') : String(turn.output_tail || '');
  }

  function firstLine(text) {
    var lines = String(text || '').split('\n');
    for (var i = 0; i < lines.length; i++) {
      var line = lines[i].replace(/^[#>*\-\s`]+/, '').trim();
      if (line) { return truncate(line, 140); }
    }
    return '';
  }

  // The kind the file list reported, else a guess from the extension
  // (mirrors DocKind::for_path for the common cases).
  function entryKind(path) {
    for (var i = 0; i < entries.length; i++) {
      if (entries[i] && entries[i].path === path && entries[i].kind) { return entries[i].kind; }
    }
    var ext = String(path).toLowerCase().split('/').pop();
    ext = ext.indexOf('.') >= 0 ? ext.split('.').pop() : '';
    if (ext === 'md' || ext === 'markdown' || ext === 'mdx') { return 'markdown'; }
    if (ext === 'html' || ext === 'htm') { return 'html'; }
    if (ext === 'pdf') { return 'pdf'; }
    if (['docx', 'xlsx', 'pptx', 'doc', 'xls', 'ppt'].indexOf(ext) >= 0) { return 'office'; }
    return 'text';
  }

  function docTurnDomId(id) {
    return 'doc-turn-' + String(id || '').replace(/[^A-Za-z0-9_-]/g, '_');
  }

  function cssEscapeAttr(value) {
    return String(value).replace(/["\\]/g, '\\$&');
  }

  // The chat <li> that shows this turn's text, if it is on screen.
  function chatMessageNode(messageId) {
    if (!messageId) { return null; }
    var transcript = document.querySelector('#session .session-transcript');
    if (!transcript) { return null; }
    return transcript.querySelector('li.msg[data-msg-id="' + cssEscapeAttr(messageId) + '"]');
  }

  function flash(node) {
    if (!node) { return; }
    node.classList.remove('flash');
    // Restart the highlight animation.
    void node.offsetWidth;
    node.classList.add('flash');
    window.setTimeout(function () { node.classList.remove('flash'); }, 1600);
  }

  // FR-1210: expand the Documents panel and bring one turn into view.
  function revealTurn(turnId) {
    if (typeof window.dmuxaiSetCenterView === 'function') { window.dmuxaiSetCenterView('diff'); }
    setDocsExpanded(true);
    writeDocsExpanded(true);
    var row = document.getElementById(docTurnDomId(turnId));
    if (!row) { return; }
    row.scrollIntoView({ block: 'center', behavior: 'smooth' });
    flash(row);
  }

  // FR-1220: open the Diff view at one file of one turn.
  function revealFile(turnId, path) {
    revealTurn(turnId);
    var row = document.getElementById(docTurnDomId(turnId));
    if (!row) { return; }
    var links = row.querySelectorAll('.doc-file');
    for (var i = 0; i < links.length; i++) {
      var a = links[i].querySelector('.doc-link');
      if (a && a.textContent === path) {
        links[i].scrollIntoView({ block: 'center' });
        flash(links[i]);
        var focusable = links[i].querySelector('a');
        if (focusable) { focusable.focus(); }
        return;
      }
    }
  }

  function revealMessage(messageId) {
    // The chat sits under the Diff view; close the view first.
    if (typeof window.dmuxaiSetCenterView === 'function') { window.dmuxaiSetCenterView(null); }
    var node = chatMessageNode(messageId);
    if (!node) { return; }
    node.scrollIntoView({ block: 'center', behavior: 'smooth' });
    flash(node);
  }

  // FR-1209: one place renders an agent turn's text. When the chat shows the
  // message, the Documents row keeps only a one-line reference to it; when
  // the message is not in the chat (graph node, older conversation, capped
  // transcript), the row shows the full tail as before.
  function fillTurnOutput(row, turn, inChat) {
    var slot = row.querySelector('.doc-turn-out');
    if (!slot) { return; }
    var mode = inChat ? 'chat' : 'tail';
    if (slot.getAttribute('data-mode') === mode) { return; }
    slot.setAttribute('data-mode', mode);
    slot.replaceChildren();
    var text = turnTailText(turn);
    if (inChat) {
      var ref = el('div', 'doc-turn-ref');
      ref.appendChild(el('span', 'doc-turn-preview', firstLine(text) || '(no text output)'));
      var show = el('button', 'btn btn-small doc-show-chat', 'Show in chat');
      show.type = 'button';
      show.addEventListener('click', function () { revealMessage(turn.message_id); });
      ref.appendChild(show);
      slot.appendChild(ref);
      return;
    }
    if (!text) { return; }
    var tail = el('pre', 'doc-tail', truncate(text, 2000));
    slot.appendChild(tail);
    // FR-749: exactly the workspace paths listed in `links` become clickable;
    // everything else in the output tail stays plain text.
    linkify(tail, Array.isArray(turn.links) ? turn.links : []);
  }

  // FR-1210: a compact document card under the chat message whose turn
  // changed files: title and type, a one-line preview and a button that opens
  // the turn in the Documents panel. The file contents stay in Documents.
  function docCard(turn) {
    var files = Array.isArray(turn.files) ? turn.files : [];
    var card = el('div', 'msg-doc-card');
    card.setAttribute('data-turn-id', String(turn.id || ''));
    var first = files[0] || {};
    var path = String(first.path || '');
    var name = path.split('/').pop() || path;
    var head = el('div', 'msg-doc-card-head');
    head.appendChild(el('span', 'msg-doc-card-icon', '\u{1F4C4}'));
    head.lastChild.setAttribute('aria-hidden', 'true');
    head.appendChild(el('span', 'msg-doc-card-title',
      files.length > 1 ? name + ' +' + (files.length - 1) + ' more' : name));
    head.appendChild(kindBadge(entryKind(path)));
    card.appendChild(head);
    // FR-1220: one muted row per changed file; a row opens the Diff view at
    // that file. The change log has no line counts, so no +N/-M is shown.
    var list = el('ul', 'msg-doc-card-files');
    var VERBS = { created: 'Created', modified: 'Edited', deleted: 'Deleted' };
    files.slice(0, 5).forEach(function (f) {
      var fpath = String(f.path || '');
      var change = String(f.change || 'modified');
      var li = el('li', 'msg-doc-card-file');
      var b = el('button', 'msg-doc-card-file-btn');
      b.type = 'button';
      b.title = 'Show ' + fpath + ' in the Diff view';
      b.appendChild(el('span', 'msg-doc-card-verb doc-chg-' + change, VERBS[change] || 'Edited'));
      b.appendChild(el('span', 'msg-doc-card-path', fpath));
      b.addEventListener('click', function () { revealFile(turn.id, fpath); });
      li.appendChild(b);
      list.appendChild(li);
    });
    if (files.length > 5) {
      list.appendChild(el('li', 'msg-doc-card-more', '+' + (files.length - 5) + ' more files'));
    }
    card.appendChild(list);
    var open = el('button', 'btn btn-small msg-doc-card-open', 'Open in Diff');
    open.type = 'button';
    open.setAttribute('aria-controls', 'docs');
    open.addEventListener('click', function () { revealTurn(turn.id); });
    card.appendChild(open);
    return card;
  }

  function syncChatLinks() {
    var wrap = $('docs-changes');
    var shown = 0;
    var cards = {};
    if (wrap) {
      var rows = wrap.querySelectorAll('li.doc-turn');
      for (var i = 0; i < rows.length; i++) {
        var row = rows[i];
        var turn = row.dmuxTurn || {};
        var files = Array.isArray(turn.files) ? turn.files : [];
        var links = Array.isArray(turn.links) ? turn.links : [];
        var msgNode = chatMessageNode(turn.message_id);
        fillTurnOutput(row, turn, !!msgNode);
        // A turn that changed nothing and whose text is in the chat has
        // nothing left to show here.
        row.hidden = !!msgNode && files.length === 0 && links.length === 0;
        if (!row.hidden) { shown++; }
        if (msgNode && files.length > 0 && !cards[turn.message_id]) {
          cards[turn.message_id] = true;
          var existing = msgNode.querySelector('.msg-doc-card');
          if (!existing || existing.getAttribute('data-turn-id') !== String(turn.id || '')) {
            if (existing) { existing.parentNode.removeChild(existing); }
            var transcript = msgNode.parentNode;
            var atEnd = transcript &&
              transcript.scrollHeight - transcript.scrollTop - transcript.clientHeight < 40;
            msgNode.appendChild(docCard(turn));
            // Keep following the latest turn when the card lands under it.
            if (atEnd) { transcript.scrollTop = transcript.scrollHeight; }
          }
        }
      }
      var empty = wrap.querySelector('.docs-none-shown');
      if (rows.length > 0 && shown === 0) {
        if (!empty) { wrap.appendChild(el('p', 'empty docs-none-shown', 'no file changes yet')); }
      } else if (empty) {
        empty.parentNode.removeChild(empty);
      }
    }
    var stale = document.querySelectorAll('#session .msg-doc-card');
    for (var j = 0; j < stale.length; j++) {
      var li = stale[j].closest('li.msg');
      if (!li || !cards[li.getAttribute('data-msg-id')]) {
        stale[j].parentNode.removeChild(stale[j]);
      }
    }
    setDocsCount(shown);
  }

  function renderChanges(turns) {
    var wrap = $('docs-changes');
    if (!wrap) { return; }
    wrap.replaceChildren();
    if (turns.length === 0) {
      wrap.appendChild(el('p', 'empty', 'no agent changes yet'));
      syncChatLinks();
      return;
    }
    var frag = document.createDocumentFragment();
    for (var i = 0; i < turns.length; i++) {
      frag.appendChild(changesRow(turns[i] || {}));
    }
    wrap.appendChild(frag);
    syncChatLinks();
  }

  // -----------------------------------------------------------------------
  // FR-749 linkification: only exact paths from `links` become anchors.
  // -----------------------------------------------------------------------

  function docLink(path) {
    var a = el('a', 'doc-link', path);
    a.href = '#doc=' + encodeURIComponent(path);
    a.addEventListener('click', function (e) {
      e.preventDefault();
      open(path);
    });
    return a;
  }

  function linkify(target, links) {
    if (!target) { return; }
    var text = target.textContent;
    var list = [];
    for (var i = 0; i < links.length; i++) {
      if (typeof links[i] === 'string' && links[i]) {
        list.push(links[i]);
      }
    }
    // Longest first so overlapping paths pick the most specific one.
    list.sort(function (a, b) { return b.length - a.length; });
    var byFirst = {};
    for (var j = 0; j < list.length; j++) {
      var c = list[j].charAt(0);
      if (!byFirst[c]) { byFirst[c] = []; }
      byFirst[c].push(list[j]);
    }

    var replaceChildren = typeof target.replaceChildren === 'function';
    if (!replaceChildren) { target.textContent = ''; }
    else { target.replaceChildren(); }

    var plain = '';
    var pos = 0;
    while (pos < text.length) {
      var match = null;
      var candidates = byFirst[text.charAt(pos)];
      if (candidates) {
        for (var k = 0; k < candidates.length; k++) {
          var cand = candidates[k];
          if (cand.length && text.slice(pos, pos + cand.length) === cand) {
            match = cand;
            break;
          }
        }
      }
      if (match) {
        if (plain) {
          target.appendChild(document.createTextNode(plain));
          plain = '';
        }
        target.appendChild(docLink(match));
        pos += match.length;
      } else {
        plain += text.charAt(pos);
        pos += 1;
      }
    }
    if (plain) { target.appendChild(document.createTextNode(plain)); }
  }

  // -----------------------------------------------------------------------
  // Serve mode (UX-753): `dmuxai serve` snaps source 'files' -> read-only.
  // -----------------------------------------------------------------------

  function applyServeState(snap) {
    if (!snap) { return; }
    applyWorkspaceSnapshot(snap);
    if (typeof snap.source !== 'string') { return; }
    readOnly = snap.source === 'files';
    readOnlyKnown = true;
    if (current && docBodies.badges) { setBadges(current); }
    updateDocControls();
  }

  function loadServeState() {
    var token = currentToken();
    if (!token) { return; }
    fetch('/api/state', {
      headers: { Authorization: 'Bearer ' + token },
      cache: 'no-store'
    })
      .then(function (res) {
        if (!res.ok) { throw new Error('HTTP ' + res.status); }
        return res.json();
      })
      .then(applyServeState)
      .catch(function () { /* unknown state keeps Edit disabled; retried below */ });
  }

  function onSnapshotEvent(e) {
    applyServeState(e && e.detail);
    // If the token came back (e.g. the page was reloaded with a fresh
    // ?token= URL), restart polling.
    if (changesTimer === null && currentToken()) { startChangesPolling(); }
    scheduleSnapshotRefresh();
  }

  // -----------------------------------------------------------------------
  // Preview pane (UX-753). Built once, shown/hidden per document.
  // -----------------------------------------------------------------------

  function btn(label, className, handler) {
    var b = document.createElement('button');
    b.type = 'button';
    b.className = className;
    b.textContent = label;
    b.addEventListener('click', handler);
    return b;
  }

  function maybeDiscard() {
    if (current && current.dirty) {
      return window.confirm(
        'Discard unsaved changes to "' + current.path + '"?'
      );
    }
    return true;
  }

  function ensurePane() {
    if (pane) { return; }
    pane = el('div', 'doc-overlay');
    pane.hidden = true;

    var frame = el('div', 'doc-pane');
    frame.setAttribute('role', 'dialog');
    frame.setAttribute('aria-modal', 'true');
    frame.setAttribute('aria-label', 'Document preview');

    var head = el('div', 'doc-pane-head');
    var title = el('span', 'doc-title');
    title.id = 'dmuxai-doc-title';
    frame.setAttribute('aria-labelledby', 'dmuxai-doc-title');
    head.appendChild(title);
    var badges = el('span', 'doc-badges');
    badges.id = 'dmuxai-doc-badges';
    head.appendChild(badges);
    head.appendChild(btn('Close', 'btn', function () { close(); }));
    frame.appendChild(head);

    docBody = el('div', 'doc-pane-body');
    // Delegated clicks for rendered markdown: relative links open here,
    // absolute (schemed) links keep the server-set new-tab behavior (FR-749).
    docBody.addEventListener('click', onMdBodyClick);
    frame.appendChild(docBody);

    var foot = el('div', 'doc-pane-foot');

    var hint = el('p', 'doc-hint', 'Read-only: start dmuxai --web to edit');
    hint.hidden = true;
    hint.id = 'dmuxai-doc-hint';
    foot.appendChild(hint);

    var conflict = el('div', 'doc-conflict');
    conflict.hidden = true;
    conflict.id = 'dmuxai-doc-conflict';
    var conflictText = el('span', 'doc-conflict-text', '');
    conflictText.id = 'dmuxai-doc-conflict-text';
    conflict.appendChild(conflictText);
    conflict.appendChild(
      btn('Reload', 'btn', function () {
        // Reload refetches the file; the local draft is replaced.
        if (!current) { return; }
        current.dirty = false;
        current.editing = false;
        current.sha256 = conflictPayload && conflictPayload.sha256
          ? String(conflictPayload.sha256)
          : null;
        conflictPayload = null;
        internalOpen(current.path);
      })
    );
    conflict.appendChild(
      btn('Overwrite', 'btn btn-primary', function () {
        var textarea = docBody.querySelector('textarea.doc-editor');
        if (!current || !textarea || !conflictPayload) { return; }
        saveWithBase(conflictPayload.sha256, textarea.value, null);
      })
    );
    conflict.appendChild(
      btn('Cancel', 'btn', function () {
        hideConflict();
      })
    );
    foot.appendChild(conflict);

    var actions = el('span', 'doc-actions');
    actions.id = 'dmuxai-doc-actions';
    actions.appendChild(btn('Edit', 'btn btn-primary', function () { startEdit(); }));
    actions.appendChild(btn('Download', 'btn', function () { downloadCurrent(); }));
    actions.appendChild(btn('Save', 'btn btn-primary', function () { save(); }));
    actions.appendChild(btn('Cancel', 'btn', function () { cancelEdit(); }));
    foot.appendChild(actions);

    var status = el('span', 'doc-status', '');
    status.id = 'dmuxai-doc-status';
    foot.appendChild(status);

    frame.appendChild(foot);
    pane.appendChild(frame);
    document.body.appendChild(pane);

    docBodies = {
      title: title,
      badges: badges,
      hint: hint,
      conflict: conflict,
      conflictText: conflictText,
      actions: actions,
      status: status
    };
  }

  function setStatus(text, isError) {
    var status = docBodies.status;
    if (!status) { return; }
    status.textContent = text || '';
    status.classList.toggle('doc-status-error', Boolean(isError));
  }

  function setBadges(doc) {
    var badges = docBodies.badges;
    badges.replaceChildren();
    badges.appendChild(kindBadge(current.kind));
    if (current.editable && !readOnly) {
      badges.appendChild(docBadge('editable', 'doc-editable-mark', 'editable in the browser'));
    } else if (current.editable) {
      badges.appendChild(docBadge('read-only', 'doc-preview-only', 'serve mode is preview-only'));
    } else {
      badges.appendChild(docBadge('Preview only', 'doc-preview-only', 'this file cannot be edited here'));
    }
  }

  function updateDocControls() {
    if (!pane || !current) { return; }
    var buttons = docBodies.actions.children;
    var edit = buttons[0];
    var download = buttons[1];
    var save = buttons[2];
    var cancel = buttons[3];
    // Findings/UX-753: Edit stays hidden until the serve/--web state is
    // known, so a serve user never even attempts the 403-able PUT.
    var editableNow = current.editable
      && !current.editing
      && readOnlyKnown
      && !readOnly;
    edit.hidden = !editableNow;
    download.hidden = !current.download;
    save.hidden = !current.editing;
    cancel.hidden = !current.editing;
    docBodies.hint.hidden = !(readOnlyKnown && readOnly && current.editable);
  }

  function showPane(visible) {
    ensurePane();
    pane.hidden = !visible;
  }

  function showLoadError(err) {
    docBody.replaceChildren();
    docBody.appendChild(el('p', 'doc-error', 'Cannot open document: ' + err.message));
    updateDocControls();
  }

  function internalOpen(path) {
    ensurePane();
    showPane(true);
    hideConflict();
    setStatus('');
    docBodies.title.textContent = path;
    docBodies.badges.replaceChildren();
    current = {
      path: path,
      kind: 'binary',
      editable: false,
      sha256: null,
      text: null,
      html: null,
      codeHtml: null,
      download: null,
      editing: false,
      dirty: false
    };
    docBody.replaceChildren();
    docBody.appendChild(el('p', 'empty', 'Loading ' + path + ' \u2026'));
    updateDocControls();

    api('GET', '/api/docs/file?path=' + encodeURIComponent(path))
      .then(function (doc) {
        current.kind = String(doc.kind || 'binary');
        current.editable = Boolean(doc.editable);
        current.sha256 = doc.sha256 ? String(doc.sha256) : null;
        current.text = typeof doc.content === 'string' ? doc.content : null;
        current.codeHtml = typeof doc.code_html === 'string' ? doc.code_html : null;
        setBadges(doc);
        renderDocument();
      })
      .catch(showLoadError);
  }

  function renderDocument() {
    hideConflict();
    setStatus('');
    if (current.kind === 'markdown') {
      refreshRender();
    } else if (current.kind === 'text') {
      showTextView();
      updateDocControls();
    } else {
      requestView();
    }
  }

  // Text kinds: the server sends `code_html` (syntax classes, one
  // .code-line per source line, numbered by a CSS counter so the numbers
  // are not copied). It goes through sanitizeInto like rendered Markdown;
  // large files have no code_html and fall back to plain text.
  function showTextView() {
    if (current.kind === 'text' && current.codeHtml) {
      var view = el('div', 'doc-code-view');
      sanitizeInto(view, current.codeHtml);
      docBody.replaceChildren();
      docBody.appendChild(view);
      return;
    }
    var pre = el('pre', 'doc-code');
    var code = el('code', null, current.text === null ? '' : current.text);
    pre.appendChild(code);
    docBody.replaceChildren();
    docBody.appendChild(pre);
  }

  var ALLOWED_TAGS = {
    p: 1, br: 1, hr: 1, h1: 1, h2: 1, h3: 1, h4: 1, h5: 1, h6: 1,
    strong: 1, em: 1, del: 1, s: 1, code: 1, pre: 1, blockquote: 1,
    ul: 1, ol: 1, li: 1, table: 1, thead: 1, tbody: 1, tr: 1, th: 1, td: 1,
    a: 1, img: 1, span: 1, div: 1, sup: 1, sub: 1, input: 1, details: 1,
    summary: 1, kbd: 1, section: 1, tfoot: 1, b: 1, i: 1, u: 1, ins: 1,
    mark: 1, dl: 1, dt: 1, dd: 1, caption: 1, small: 1, abbr: 1, cite: 1, q: 1
  };
  // Disallowed elements are unwrapped (text kept) except these, which are
  // dropped with their whole subtree.
  var DROPPED_TAGS = {
    script: 1, style: 1, svg: 1, math: 1, iframe: 1, object: 1, embed: 1,
    template: 1, noscript: 1, textarea: 1, select: 1, form: 1, title: 1,
    head: 1, link: 1, meta: 1, base: 1, frame: 1, frameset: 1
  };
  var SANITIZER_CLASS_RE = /^(hl-|language-|footnote|code-(line|view)$)/;
  var SANITIZER_ID_RE = /^user-content-/;

  function isSafeUrl(value, img) {
    if (value == null) return false;
    // Strip whitespace/control characters browsers ignore inside schemes.
    var v = String(value).replace(/[\u0000-\u0020\u007f-\u009f]/g, '');
    if (!v) return false;
    if (v.charAt(0) === '#') return true;
    if (/^(https?:\/\/|mailto:)/i.test(v)) return true;
    if (img && /^data:image\/(png|gif|jpeg|webp);base64,/i.test(v)) return true;
    // Relative paths only: no scheme, no protocol-relative `//host`.
    return /^(\.{0,2}\/(?!\/)|[^:/?#]+(\/|$|[?#]))/.test(v) && !/^\/\//.test(v);
  }

  function safeClasses(val) {
    return String(val).split(/\s+/).filter(function (token) {
      return token && SANITIZER_CLASS_RE.test(token);
    }).join(' ');
  }

  function copySafeAttributes(src, dest, tag) {
    var attrs = src.attributes;
    for (var i = 0; i < attrs.length; i++) {
      var name = attrs[i].name.toLowerCase();
      var val = attrs[i].value;
      if (name.indexOf('on') === 0) continue;
      if (tag === 'a' && name === 'href' && isSafeUrl(val, false)) {
        dest.setAttribute('href', val);
      } else if (tag === 'a' && (name === 'rel' || name === 'target' || name === 'title')) {
        dest.setAttribute(name, val);
      } else if (tag === 'img' && name === 'src' && isSafeUrl(val, true)) {
        dest.setAttribute('src', val);
      } else if (tag === 'img' && (name === 'alt' || name === 'title')) {
        dest.setAttribute(name, val);
      } else if ((tag === 'th' || tag === 'td') && name === 'align') {
        if (/^(left|center|right)$/.test(val)) dest.setAttribute('align', val);
      } else if (name === 'class') {
        var classes = safeClasses(val);
        if (classes) dest.setAttribute('class', classes);
      } else if (name === 'id' && SANITIZER_ID_RE.test(val)) {
        dest.setAttribute('id', val);
      } else if (tag === 'input' && (name === 'type' || name === 'checked')) {
        if (name === 'type' && val === 'checkbox') dest.setAttribute('type', 'checkbox');
        if (name === 'checked') dest.setAttribute('checked', '');
      } else if (name === 'title') {
        dest.setAttribute('title', val);
      }
    }
    if (tag === 'input') {
      dest.setAttribute('type', 'checkbox');
      dest.setAttribute('disabled', '');
    }
  }

  function appendSanitizedChildren(src, dest) {
    var child = src.firstChild;
    while (child) {
      var next = child.nextSibling;
      var safe = sanitizeNode(child);
      if (safe) dest.appendChild(safe);
      child = next;
    }
  }

  function sanitizeNode(node) {
    if (node.nodeType === Node.TEXT_NODE) {
      return document.createTextNode(node.textContent);
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return null;
    var tag = node.tagName.toLowerCase();
    if (DROPPED_TAGS[tag] || node.namespaceURI !== 'http://www.w3.org/1999/xhtml') return null;
    if (!ALLOWED_TAGS[tag]) {
      var frag = document.createDocumentFragment();
      appendSanitizedChildren(node, frag);
      return frag;
    }
    var el = document.createElement(tag);
    copySafeAttributes(node, el, tag);
    appendSanitizedChildren(node, el);
    return el;
  }

  // Defense in depth: only allowlisted markup from the server reaches the DOM.
  function sanitizeInto(container, html) {
    container.replaceChildren();
    var tpl = document.createElement('template');
    tpl.innerHTML = String(html || '');
    appendSanitizedChildren(tpl.content, container);
  }

  // FR-743: server HTML is inserted only through sanitizeInto. The .md-body
  // wrapper is kept so GitHub-like typography rules in docs.css apply.
  function applyRenderedHtml(target, html) {
    target.replaceChildren();
    var body = document.createElement('div');
    body.className = 'md-body';
    sanitizeInto(body, html);
    target.appendChild(body);
  }

  function refreshRender() {
    api('GET', '/api/docs/render?path=' + encodeURIComponent(current.path))
      .then(function (res) {
        current.html = res && typeof res.html === 'string' ? res.html : '';
        if (!current.editing) {
          docBody.replaceChildren();
          applyRenderedHtml(docBody, current.html);
        }
        updateDocControls();
      })
      .catch(function (err) {
        current.html = null;
        if (!current.editing) {
          showTextView();
        }
        setStatus('Render failed: ' + err.message + ' (showing plain text)', true);
        updateDocControls();
      });
  }

  function requestView() {
    api('POST', '/api/docs/view', { path: current.path })
      .then(function (res) {
        var preview = res && res.preview ? String(res.preview) : 'download';
        docBody.replaceChildren();
        if (preview === 'frame' || preview === 'pdf') {
          if (current.kind === 'html') {
            showHtmlFrame(res.url);
          } else {
            showPdfFrame(res.url, res.download);
          }
        } else if (preview === 'unavailable') {
          showUnavailable(res.reason, res.download);
        } else {
          showDownloadView(res.download);
        }
        updateDocControls();
      })
      .catch(showLoadError);
  }

  // FR-746/OQ-741: the html frame runs without scripts and without same-origin
  // powers; the server additionally answers with a sandbox CSP.
  function showHtmlFrame(url) {
    var iframe = document.createElement('iframe');
    iframe.className = 'doc-frame';
    iframe.setAttribute('sandbox', '');
    iframe.src = ticketUrl(url);
    iframe.title = current.path;
    iframe.setAttribute('referrerpolicy', 'no-referrer');
    docBody.replaceChildren();
    docBody.appendChild(iframe);
  }

  // PDF frames are intentionally NOT sandboxed: Chrome's built-in PDF viewer
  // refuses to render PDFs inside sandboxed frames, so the empty sandbox the
  // HTML preview uses would show an error page instead. The frame only ever
  // points at a ticket URL for a LibreOffice-generated conversion or a plain
  // workspace PDF; the ticket grants those bytes only, carries no dashboard
  // credentials, and expires, so rendering scripts-free content inline is
  // the safe exception here (FR-746/FR-747).
  function showPdfFrame(url, download) {
    var iframe = document.createElement('iframe');
    iframe.className = 'doc-frame';
    iframe.src = ticketUrl(url);
    iframe.title = current.path;
    docBody.replaceChildren();
    docBody.appendChild(iframe);
    if (download) {
      current.download = String(download);
    }
  }

  function showUnavailable(reason, download) {
    docBody.replaceChildren();
    var wrap = el('div', 'doc-dl');
    wrap.appendChild(el('p', 'doc-error', 'Preview unavailable: ' + String(reason || 'unknown reason')));
    if (download) {
      current.download = String(download);
      wrap.appendChild(downloadAnchor());
    }
    docBody.appendChild(wrap);
  }

  function showDownloadView(download) {
    docBody.replaceChildren();
    var wrap = el('div', 'doc-dl');
    wrap.appendChild(el('p', 'doc-error', 'Preview not available for this file type.'));
    if (download) {
      current.download = String(download);
      wrap.appendChild(downloadAnchor());
    }
    docBody.appendChild(wrap);
  }

  function downloadAnchor() {
    var a = el('a', 'btn', 'Download');
    a.href = ticketUrl(current.download);
    a.setAttribute('target', '_blank');
    a.setAttribute('rel', 'noopener noreferrer');
    return a;
  }

  function downloadCurrent() {
    if (!current || !current.download) { return; }
    var a = document.createElement('a');
    a.href = ticketUrl(current.download);
    a.setAttribute('target', '_blank');
    a.setAttribute('rel', 'noopener noreferrer');
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
  }

  // -----------------------------------------------------------------------
  // Editing (UX-753): markdown split view with live preview; plain text
  // without preview. Concurrency: base_sha256 -> 409 conflict notice.
  // -----------------------------------------------------------------------

  function startEdit() {
    if (!current || !current.editable || !readOnlyKnown || readOnly || current.editing) {
      return;
    }
    current.editing = true;
    current.dirty = false;
    hideConflict();
    setStatus('');
    docBody.replaceChildren();
    var editor = document.createElement('textarea');
    editor.className = 'doc-editor';
    editor.value = current.text === null ? '' : current.text;
    editor.spellcheck = false;
    editor.addEventListener('input', function () {
      current.dirty = true;
      setStatus('');
      if (current.kind === 'markdown') { scheduleLiveRender(editor.value); }
    });
    if (current.kind === 'markdown') {
      var grid = el('div', 'doc-edit-grid');
      grid.appendChild(editor);
      var live = el('div', 'md-body md-live', '');
      live.id = 'dmuxai-doc-live';
      grid.appendChild(live);
      docBody.appendChild(grid);
      applyRenderedHtml(live, current.html || '');
    } else {
      editor.classList.add('doc-editor-single');
      docBody.appendChild(editor);
    }
    editor.focus();
    updateDocControls();
  }

  function cancelEdit() {
    if (!current || !current.editing) { return; }
    if (current.dirty && !window.confirm(
      'Discard unsaved changes to "' + current.path + '"?'
    )) {
      return;
    }
    current.editing = false;
    clearRenderTimer();
    restoreView();
    updateDocControls();
  }

  function restoreView() {
    docBody.replaceChildren();
    setStatus('');
    hideConflict();
    if (current.kind === 'markdown') {
      if (current.html !== null) {
        applyRenderedHtml(docBody, current.html);
      } else {
        showTextView();
      }
    } else {
      showTextView();
    }
  }

  function scheduleLiveRender(text) {
    if (renderTimer !== null) {
      window.clearTimeout(renderTimer);
    }
    renderTimer = window.setTimeout(function () {
      renderTimer = null;
      api('POST', '/api/docs/render', { text: text })
        .then(function (res) {
          var live = $('dmuxai-doc-live');
          if (live) {
            applyRenderedHtml(live, res && typeof res.html === 'string' ? res.html : '');
          }
        })
        .catch(function (err) {
          setStatus('Preview failed: ' + err.message, true);
        });
    }, RENDER_DEBOUNCE_MS);
  }

  function clearRenderTimer() {
    if (renderTimer !== null) {
      window.clearTimeout(renderTimer);
      renderTimer = null;
    }
  }

  function save() {
    if (!current || !current.editing) { return; }
    var textarea = docBody.querySelector('textarea.doc-editor');
    if (!textarea) { return; }
    var base = current.sha256;
    if (base === null) {
      setStatus('Cannot save: missing file hash (reload the document).', true);
      return;
    }
    saveWithBase(base, textarea.value, null);
  }

  // FR-744: the save is guarded by the hash of the content we loaded; a stale
  // hash answers 409 and surfaces the conflict notice with Reload/Overwrite.
  function saveWithBase(base, content, forceBase) {
    var usedBase = forceBase !== null && forceBase !== undefined ? forceBase : base;
    setStatus('Saving\u2026');
    api('PUT', '/api/docs/file', {
      path: current.path,
      content: content,
      base_sha256: usedBase
    })
      .then(function (res) {
        current.sha256 = res && res.sha256 ? String(res.sha256) : usedBase;
        current.text = content;
        // The old code view no longer matches; show plain text until the
        // highlighted view of the saved content arrives.
        current.codeHtml = null;
        if (current.kind === 'text') { refreshCodeView(current.path, current.sha256); }
        current.dirty = false;
        hideConflict();
        setStatus('Saved.');
        if (current.editing) {
          exitEditingAfterSave();
        }
        refreshList();
        refreshChanges();
      })
      .catch(function (err) {
        if (err.status === 409 && err.payload) {
          conflictPayload = err.payload;
          docBodies.conflictText.textContent =
            'Conflict: the file changed on disk while you were editing. ' +
            'Reload to see the file, or overwrite it with your version.';
          docBodies.conflict.hidden = false;
          setStatus('');
        } else {
          setStatus('Save failed: ' + err.message, true);
        }
      });
  }

  function refreshCodeView(path, sha256) {
    api('GET', '/api/docs/file?path=' + encodeURIComponent(path))
      .then(function (doc) {
        if (!current || current.path !== path || current.sha256 !== sha256) { return; }
        if (String(doc.sha256 || '') !== sha256) { return; }
        current.codeHtml = typeof doc.code_html === 'string' ? doc.code_html : null;
        if (!current.editing && current.codeHtml) { showTextView(); }
      })
      .catch(function () { /* keep the plain text view */ });
  }

  function exitEditingAfterSave() {
    current.editing = false;
    clearRenderTimer();
    if (current.kind === 'markdown') {
      refreshRender();
    } else {
      restoreView();
      updateDocControls();
    }
  }

  function hideConflict() {
    if (!pane || !conflictPayload) {
      if (docBodies.conflict) { docBodies.conflict.hidden = true; }
      return;
    }
    conflictPayload = null;
    docBodies.conflict.hidden = true;
  }

  // Markdown body link routing (FR-749): only links inside a rendered
  // .md-body are routed; everything else in the pane body (the download
  // anchor with its /docs/view or /docs/download ticket URL, error
  // fallbacks) keeps the browser default behavior. Relative workspace
  // paths open in the pane; schemed links keep the server-set new tab.
  function onMdBodyClick(e) {
    if (e.defaultPrevented) { return; }
    var target = e.target;
    var a = target && target.closest ? target.closest('a') : null;
    if (!a || !docBody.contains(a)) { return; }
    if (!a.closest('.md-body')) { return; }
    var href = a.getAttribute('href') || '';
    if (!href || href.charAt(0) === '#') { return; }
    if (/^[a-zA-Z][a-zA-Z0-9+.\-]*:/.test(href)) { return; }
    e.preventDefault();
    var path = href.split('#')[0];
    if (path.charAt(0) === '/') { path = path.slice(1); }
    path = path.replace(/^\.\//, '');
    if (!path) { return; }
    try { path = decodeURIComponent(path); } catch (err) { /* keep raw */ }
    open(path);
  }

  // -----------------------------------------------------------------------
  // Deep links (FR-749): '#doc=<encoded path>' opens the document.
  // -----------------------------------------------------------------------

  function hashDocPath() {
    var h = window.location.hash || '';
    if (h.indexOf('#doc=') !== 0) { return null; }
    var raw = h.slice(5);
    try {
      return decodeURIComponent(raw);
    } catch (err) {
      return null;
    }
  }

  function onHashChange() {
    var path = hashDocPath();
    if (path && (!current || current.path !== path)) {
      if (!maybeDiscard()) { return; }
      internalOpen(path);
    }
  }

  function open(path) {
    if (typeof path !== 'string' || !path) { return; }
    // Same-document request: a no-op, so never prompt about unsaved edits.
    if (current && current.path === path) { return; }
    if (!maybeDiscard()) { return; }
    var hash = '#doc=' + encodeURIComponent(path);
    if (window.location.hash === hash) {
      internalOpen(path);
    } else {
      window.location.hash = hash;
    }
  }

  function close() {
    if (!pane || pane.hidden) {
      if (current) {
        if (!maybeDiscard()) { return; }
        current = null;
        clearRenderTimer();
      }
      return;
    }
    if (!maybeDiscard()) { return; }
    current = null;
    conflictPayload = null;
    clearRenderTimer();
    pane.hidden = true;
    try {
      window.history.replaceState(
        null, '',
        window.location.pathname + window.location.search
      );
    } catch (err) {
      try { window.location.hash = ''; } catch (err2) { /* ignore */ }
    }
  }

  // -----------------------------------------------------------------------
  // Init. INT-754: the module is exposed even without a token so other
  // panels keep working.
  // -----------------------------------------------------------------------

  window.DmuxDocs = {
    open: open,
    close: close,
    linkify: linkify,
    changes: changes
  };

  // UX-753: closing the tab or reloading while the editor has unsaved
  // changes must prompt, same as in-pane close/switch.
  function onBeforeUnload(e) {
    if (current && current.dirty) {
      e.preventDefault();
      e.returnValue = '';
    }
  }

  function init() {
    initDocsToggle();
    // session.js fires this after every chat render (new or streamed messages).
    window.addEventListener('dmuxai:transcript-rendered', syncChatLinks);
    initSidebar();
    bindFilter();
    if (!currentToken()) {
      refreshList();
      window.addEventListener('beforeunload', onBeforeUnload);
      window.addEventListener(SNAPSHOT_EVENT, onSnapshotEvent);
      return;
    }
    refreshList();
    loadServeState();
    startChangesPolling();
    window.addEventListener('hashchange', onHashChange);
    window.addEventListener('beforeunload', onBeforeUnload);
    window.addEventListener(SNAPSHOT_EVENT, onSnapshotEvent);
    document.addEventListener('keydown', function (e) {
      if (!pane || pane.hidden || !current || !current.editing) { return; }
      var key = typeof e.key === 'string' ? e.key.toLowerCase() : '';
      if ((e.ctrlKey || e.metaKey) && !e.altKey && key === 's') {
        e.preventDefault();
        save();
      }
    });
    onHashChange();
  }

  init();
})();
