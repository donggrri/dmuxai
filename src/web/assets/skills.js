'use strict';

// SPEC-stage-skills UX-1500: stage -> skill map in the dashboard.
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

  var toggle = document.getElementById('skills-toggle');
  var root = document.getElementById('skills-popover');
  if (!toggle || !root) { return; }

  function statusFor(rows, stage, skill) {
    for (var i = 0; i < rows.length; i++) {
      if (rows[i].stage === stage && rows[i].skill === skill) { return rows[i]; }
    }
    return null;
  }

  function render(data, error) {
    root.replaceChildren();
    root.appendChild(el('h2', 'memory-title', 'Stage skills'));
    if (error) { root.appendChild(el('p', 'memory-error', error)); }
    var rows = (data && data.rows) || [];
    var stages = (data && data.stages) || [];
    var table = el('table', 'skills-table');
    var head = el('tr');
    ['Stage', 'Skills (comma separated)', 'Status', ''].forEach(function (t) { head.appendChild(el('th', '', t)); });
    table.appendChild(head);
    stages.forEach(function (entry) {
      var tr = el('tr');
      tr.appendChild(el('td', 'skills-stage', entry.stage));
      var input = el('input', 'skills-input');
      input.type = 'text';
      input.value = (entry.skills || []).join(', ');
      input.setAttribute('aria-label', 'Skills for ' + entry.stage);
      var tdIn = el('td');
      tdIn.appendChild(input);
      tr.appendChild(tdIn);
      var status = (entry.skills || []).map(function (skill) {
        var row = statusFor(rows, entry.stage, skill);
        return skill + ': ' + (row ? row.status : '?') + (row && row.origin ? ' (' + row.origin + ')' : '');
      }).join('; ');
      tr.appendChild(el('td', 'skills-status', status));
      var save = el('button', 'btn', 'Save');
      save.type = 'button';
      save.addEventListener('click', function () {
        var list = input.value.split(',').map(function (s) { return s.trim(); }).filter(Boolean);
        if (!window.confirm('Write [stages.' + entry.stage + '] skills = [' + list.join(', ') + '] to dmuxai.toml?')) { return; }
        api('POST', '/api/skills', { stage: entry.stage, skills: list })
          .then(function (d) { render(d, null); })
          .catch(function (err) { render(data, String(err.message || err)); });
      });
      var tdBtn = el('td');
      tdBtn.appendChild(save);
      tr.appendChild(tdBtn);
      table.appendChild(tr);
    });
    root.appendChild(table);
    rows.filter(function (r) { return r.status === 'needs approval'; }).forEach(function (r) {
      var line = el('p', 'skills-approve');
      line.appendChild(document.createTextNode(r.skill + ': pinned commit changed or new. '));
      var btn = el('button', 'btn btn-primary', 'Approve ' + r.skill);
      btn.type = 'button';
      btn.addEventListener('click', function () {
        if (!window.confirm('Approve the pinned commit of ' + r.skill + '? Its SKILL.md will be sent to agents.')) { return; }
        api('POST', '/api/skills/approve', { source: r.skill })
          .then(function (d) { render(d, null); })
          .catch(function (err) { render(data, String(err.message || err)); });
      });
      line.appendChild(btn);
      root.appendChild(line);
    });
    root.appendChild(el('p', 'memory-hint', 'Git sources: dmuxai skills sync, then /skills approve <source>. Changes apply to the next prompt.'));
  }

  function load() {
    api('GET', '/api/skills').then(function (d) { render(d, null); })
      .catch(function (err) { render(null, String(err.message || err)); });
  }

  toggle.addEventListener('click', function () {
    var open = root.hidden;
    root.hidden = !open;
    toggle.setAttribute('aria-expanded', open ? 'true' : 'false');
    if (open) { load(); root.focus(); }
  });
  document.addEventListener('keydown', function (event) {
    if (event.key === 'Escape' && !root.hidden) {
      root.hidden = true;
      toggle.setAttribute('aria-expanded', 'false');
    }
  });
})();
