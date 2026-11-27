'use strict';

/* Read-only Markdown file path linkifier and viewer modal (FR-3216, FR-3217). */
(function () {
  var TOKEN_KEY = 'dmuxai.token';
  var SKIP_TAGS = { A: true, SCRIPT: true, STYLE: true, TEXTAREA: true, INPUT: true };

  var backdrop = null;
  var dialogCard = null;
  var titleEl = null;
  var closeBtn = null;
  var bodyEl = null;
  var lastTrigger = null;
  var requestSeq = 0;

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

    dialogCard.appendChild(header);
    dialogCard.appendChild(bodyEl);
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

  function openMdViewer(path, triggerEl) {
    if (typeof path !== 'string' || !path) { return; }
    ensureModal();
    lastTrigger = triggerEl || document.activeElement;
    titleEl.textContent = path;
    showStatus('Loading\u2026', false);
    backdrop.hidden = false;
    document.body.classList.add('md-viewer-open');
    if (closeBtn && typeof closeBtn.focus === 'function') {
      try { closeBtn.focus(); } catch (err) { /* ignore */ }
    }

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
