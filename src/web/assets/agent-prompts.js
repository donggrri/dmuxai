'use strict';

// PR-2.5 part 4: per-agent system prompts in the dashboard.
(function () {
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
    var headers = { Authorization: 'Bearer ' + token };
    var init = { method: method, headers: headers, cache: 'no-store' };
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(body);
    }
    return fetch(path, init).then(function (res) {
      return res.json().catch(function () { return null; }).then(function (payload) {
        if (!res.ok) {
          throw new Error(payload && payload.error ? payload.error : 'HTTP ' + res.status);
        }
        return payload;
      });
    });
  }

  var toggle = document.getElementById('agent-prompts-toggle');
  var root = document.getElementById('agent-prompts-popover');
  if (!toggle || !root) { return; }

  function rowClass(row) { return 'agent-prompt-row system-prompt-row'; }

  function render(data, error) {
    root.replaceChildren();
    root.appendChild(el('h2', 'memory-title', 'System prompts'));
    if (error) { root.appendChild(el('p', 'memory-error', error)); }
    var runners = (data && data.runners) || [];
    var def = (data && data.default) || null;
    var all = runners.slice();
    if (def) { all = [def].concat(all); }
    var list = el('div', 'agent-prompts-list');
    all.forEach(function (row) {
      var card = el('div', rowClass(row));
      card.setAttribute('data-runner', row.runner);
      card.setAttribute('data-set', row.set ? 'true' : 'false');
      card.appendChild(el('div', 'agent-prompt-runner', row.runner));
      card.appendChild(el('span', 'agent-prompt-set', row.set ? 'set: true' : 'set: false'));
      card.appendChild(el('span', 'agent-prompt-source', row.source ? ('source: ' + row.source) : 'source: (none)'));
      card.appendChild(el('span', 'agent-prompt-hash', row.hash ? ('hash: ' + row.hash) : 'hash: (none)'));
      var badge = el('span', 'agent-prompt-delivery', row.delivery);
      badge.classList.add(row.delivery === 'native' ? 'native' : 'prepend');
      card.appendChild(badge);
      card.appendChild(el('span', 'agent-prompt-preview', row.preview || ''));
      // editor
      var editor = el('div', 'agent-prompt-editor');
      var ta = el('textarea', 'agent-prompt-textarea');
      ta.value = '';
      // keep full not available; user types new text
      ta.setAttribute('aria-label', 'System prompt for ' + row.runner);
      var sel = el('select', 'agent-prompt-inject');
      ['session', 'always'].forEach(function (v) {
        var opt = el('option', '', v);
        opt.value = v;
        if (row.inject === v) { opt.selected = true; }
        sel.appendChild(opt);
      });
      var save = el('button', 'btn btn-primary', 'Save');
      save.type = 'button';
      var clear = el('button', 'btn', 'Clear');
      clear.type = 'button';
      save.addEventListener('click', function () {
        api('POST', '/api/agent-prompts', {
          runner: row.runner,
          system_prompt: ta.value,
          prompt_inject: sel.value,
        }).then(function (d) { render(d, null); })
          .catch(function (err) { render(data, String(err.message || err)); });
      });
      clear.addEventListener('click', function () {
        api('POST', '/api/agent-prompts', {
          runner: row.runner,
          system_prompt: null,
          prompt_inject: sel.value,
        }).then(function (d) { render(d, null); })
          .catch(function (err) { render(data, String(err.message || err)); });
      });
      editor.appendChild(ta);
      editor.appendChild(sel);
      editor.appendChild(save);
      editor.appendChild(clear);
      card.appendChild(editor);
      list.appendChild(card);
    });
    root.appendChild(list);
    if (data && data.warnings && data.warnings.length) {
      data.warnings.forEach(function (w) { root.appendChild(el('p', 'memory-error', w)); });
    }
  }

  function load() {
    api('GET', '/api/agent-prompts').then(function (d) { render(d, null); })
      .catch(function (err) { render(null, String(err.message || err)); });
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
    if (open) { load(); root.focus(); }
  });
  document.addEventListener('keydown', function (event) {
    if (event.key === 'Escape' && !root.hidden) {
      setOpen(false);
    }
  });
})();
