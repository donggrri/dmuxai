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

  function fmtSize(bytes) {
    if (typeof bytes !== 'number' || bytes < 0 || !isFinite(bytes)) {
      return '';
    }
    if (bytes < 1024) { return bytes + ' B'; }
    if (bytes < 1024 * 1024) { return (bytes / 1024).toFixed(1) + ' KB'; }
    return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
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
  // Document list (FR-741 over GET /api/docs).
  // -----------------------------------------------------------------------

  function refreshList() {
    var list = $('docs-list');
    if (!list) { return; }
    if (!currentToken()) {
      list.replaceChildren();
      list.appendChild(
        el('p', 'empty', 'documents need the dashboard token (?token= URL)')
      );
      return;
    }
    api('GET', '/api/docs')
      .then(function (payload) {
        entries = payload && Array.isArray(payload.entries) ? payload.entries : [];
        listTruncated = Boolean(payload && payload.truncated);
        renderList();
      })
      .catch(function (err) {
        renderList();
        list.replaceChildren();
        list.appendChild(
          el('p', 'empty', 'documents unavailable: ' + err.message)
        );
      });
  }

  // Only same-origin ticket URLs from the docs API reach iframe/anchor sinks.
  function ticketUrl(url) {
    var value = String(url || '');
    return /^\/docs\/(view|download)\/[A-Za-z0-9-]+$/.test(value) ? value : 'about:blank';
  }

  function docRow(entry) {
    var path = String(entry.path || '');
    var li = el('li', 'doc-row');
    li.appendChild(kindBadge(entry.kind));
    var link = docLink(path);
    li.appendChild(link);
    if (entry.editable && !readOnly) {
      li.appendChild(docBadge('editable', 'doc-editable-mark', 'editable in the browser'));
    }
    var size = fmtSize(entry.size);
    if (size) {
      var sizeSpan = el('span', 'doc-size', size);
      if (typeof entry.mtime_ms === 'number') {
        li.title = new Date(entry.mtime_ms).toLocaleString();
      }
      li.appendChild(sizeSpan);
    }
    return li;
  }

  function renderList() {
    var list = $('docs-list');
    if (!list) { return; }
    list.replaceChildren();
    var needle = filterText.toLowerCase();
    var shown = 0;
    var frag = document.createDocumentFragment();
    for (var i = 0; i < entries.length; i++) {
      var entry = entries[i] || {};
      var path = String(entry.path || '');
      if (needle && path.toLowerCase().indexOf(needle) === -1) {
        continue;
      }
      frag.appendChild(docRow(entry));
      shown++;
    }
    if (shown === 0) {
      list.appendChild(el('p', 'empty', 'no documents' + (needle ? ' match the filter' : ' yet')));
      return;
    }
    list.appendChild(frag);
    if (listTruncated) {
      list.appendChild(el('p', 'doc-note', 'list truncated at 5000 files'));
    }
  }

  function bindFilter() {
    var input = $('docs-filter');
    if (!input) { return; }
    input.addEventListener('input', function () {
      filterText = input.value;
      renderList();
    });
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

  function refreshChanges() {
    changes(null)
      .then(renderChanges)
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

    // FR-748: output_tail is a JSON array of lines; join with real line
    // breaks so the tail reads as the agent's output, then linkify.
    var tailLines = Array.isArray(turn.output_tail) ? turn.output_tail : [];
    var tailText = tailLines.length
      ? tailLines.join('\n')
      : String(turn.output_tail || '');
    var tail = el('pre', 'doc-tail', truncate(tailText, 2000));
    li.appendChild(tail);
    // FR-749: exactly the workspace paths listed in `links` become clickable;
    // everything else in the output tail stays plain text.
    linkify(tail, Array.isArray(turn.links) ? turn.links : []);
    return li;
  }

  function renderChanges(turns) {
    var wrap = $('docs-changes');
    if (!wrap) { return; }
    wrap.replaceChildren();
    if (turns.length === 0) {
      wrap.appendChild(el('p', 'empty', 'no agent changes yet'));
      return;
    }
    var frag = document.createDocumentFragment();
    for (var i = 0; i < turns.length; i++) {
      frag.appendChild(changesRow(turns[i] || {}));
    }
    wrap.appendChild(frag);
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
    if (!snap || typeof snap.source !== 'string') { return; }
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

  function showTextView() {
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
  var SANITIZER_CLASS_RE = /^(hl-|language-|footnote)/;
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
