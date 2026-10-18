'use strict';
// SPEC-sidebar-resize FR-970..FR-974, UX-970, UX-971: drag handles for the
// docked Sessions (left) and Files (right) sidebars. Sets
// --left-sidebar-width / --right-sidebar-width on <html>. Open/close stays
// in session.js and docs.js. At 900px and below this script does not write.

(function () {
  var LEFT_KEY = 'dmuxai.sidebar.width.left';
  var RIGHT_KEY = 'dmuxai.sidebar.width.right';
  var MIN_WIDTH = 180;
  var DEFAULT_WIDTH = 320;
  var NARROW_MAX = 900;
  var CONTENT_RESERVE = 360;
  var STEP = 16;
  var SHIFT_STEP = 64;
  var DBLCLICK_MS = 500;

  var preferred = { left: DEFAULT_WIDTH, right: DEFAULT_WIDTH };
  var shown = { left: DEFAULT_WIDTH, right: DEFAULT_WIDTH };
  var drag = null;
  var lastClick = null;

  function storageGet(key) {
    try { return window.localStorage.getItem(key); } catch (err) { return null; }
  }

  function storageSet(key, value) {
    try { window.localStorage.setItem(key, String(value)); } catch (err) { /* ignore */ }
  }

  function storageRemove(key) {
    try { window.localStorage.removeItem(key); } catch (err) { /* ignore */ }
  }

  // Missing or non-numeric values fall back to the default. Out-of-range
  // numbers are returned and clamped when painted.
  function storedWidth(key) {
    var raw = storageGet(key);
    if (raw == null) { return null; }
    var text = String(raw).replace(/^\s+|\s+$/g, '');
    if (!/^-?\d+$/.test(text)) { return null; }
    var n = parseInt(text, 10);
    if (!isFinite(n)) { return null; }
    return n;
  }

  function isNarrow() {
    return window.innerWidth <= NARROW_MAX;
  }

  function isLeftOpen() {
    return document.body.classList.contains('left-sidebar-open');
  }

  function isRightOpen() {
    return document.body.classList.contains('sidebar-open');
  }

  function keyFor(side) {
    return side === 'left' ? LEFT_KEY : RIGHT_KEY;
  }

  function handleFor(side) {
    return document.getElementById(side === 'left' ? 'sessions-resize' : 'sidebar-resize');
  }

  // max = max(180, min(floor(viewport * 50%), viewport - otherOpenWidth - 360))
  function maxFor(otherWidth) {
    var vw = window.innerWidth;
    var other = otherWidth > 0 ? otherWidth : 0;
    var cap = Math.min(Math.floor(vw * 0.5), vw - other - CONTENT_RESERVE);
    if (cap < MIN_WIDTH) { return MIN_WIDTH; }
    return cap;
  }

  function clamp(value, max) {
    var n = Math.round(value);
    if (n < MIN_WIDTH) { return MIN_WIDTH; }
    if (n > max) { return max; }
    return n;
  }

  function otherShown(side) {
    if (side === 'left') {
      return isRightOpen() ? shown.right : 0;
    }
    return isLeftOpen() ? shown.left : 0;
  }

  function paint() {
    var root = document.documentElement;
    root.style.setProperty('--left-sidebar-width', shown.left + 'px');
    root.style.setProperty('--right-sidebar-width', shown.right + 'px');
    writeAria('left');
    writeAria('right');
  }

  function writeAria(side) {
    var handle = handleFor(side);
    if (!handle) { return; }
    var max = maxFor(otherShown(side));
    handle.setAttribute('aria-valuemin', String(MIN_WIDTH));
    handle.setAttribute('aria-valuemax', String(max));
    handle.setAttribute('aria-valuenow', String(side === 'left' ? shown.left : shown.right));
  }

  // Re-clamp from the stored preference. A closed sidebar keeps its own
  // width (it reserves no space); both open sidebars share the viewport.
  // Does not write storage.
  function relayout() {
    if (isNarrow()) { return; }
    if (drag) { return; }
    var leftOpen = isLeftOpen();
    var rightOpen = isRightOpen();
    var left = clamp(preferred.left, maxFor(0));
    var right = clamp(preferred.right, maxFor(0));
    var i;
    if (leftOpen && rightOpen) {
      for (i = 0; i < 3; i++) {
        left = clamp(preferred.left, maxFor(right));
        right = clamp(preferred.right, maxFor(left));
      }
    } else if (leftOpen) {
      left = clamp(preferred.left, maxFor(0));
    } else if (rightOpen) {
      right = clamp(preferred.right, maxFor(0));
    }
    shown.left = left;
    shown.right = right;
    paint();
  }

  // User gesture: change one sidebar only, clamped against the other if it is open.
  function commit(side, value, persist) {
    if (isNarrow()) { return; }
    var next = clamp(value, maxFor(otherShown(side)));
    preferred[side] = next;
    shown[side] = next;
    paint();
    if (persist) { storageSet(keyFor(side), next); }
  }

  function reset(side) {
    if (isNarrow()) { return; }
    storageRemove(keyFor(side));
    preferred[side] = DEFAULT_WIDTH;
    shown[side] = clamp(DEFAULT_WIDTH, maxFor(otherShown(side)));
    paint();
  }

  function widthFromPointer(side, clientX) {
    // Left sidebar is docked at x = 0, so its width is clientX.
    // Right sidebar is docked at the viewport's right edge.
    if (side === 'left') { return Math.round(clientX); }
    return Math.round(window.innerWidth - clientX);
  }

  function onPointerDown(side, e) {
    if (drag) { return; }
    if (typeof e.button === 'number' && e.button !== 0) { return; }
    if (isNarrow()) { return; }
    if (e.preventDefault) { e.preventDefault(); }
    drag = {
      side: side,
      pointerId: e.pointerId,
      startX: e.clientX,
      moved: false
    };
    document.body.classList.add('sidebar-resizing');
    var handle = handleFor(side);
    if (handle) {
      handle.classList.add('is-dragging');
      if (handle.setPointerCapture) {
        try { handle.setPointerCapture(e.pointerId); } catch (err) { /* ignore */ }
      }
    }
  }

  function onPointerMove(e) {
    if (!drag || e.pointerId !== drag.pointerId) { return; }
    if (Math.abs(e.clientX - drag.startX) > 3) { drag.moved = true; }
    commit(drag.side, widthFromPointer(drag.side, e.clientX), false);
  }

  function finishDrag(e, persist) {
    if (!drag) { return; }
    if (e && e.pointerId !== drag.pointerId) { return; }
    var side = drag.side;
    var moved = drag.moved;
    var handle = handleFor(side);
    drag = null;
    document.body.classList.remove('sidebar-resizing');
    if (handle) { handle.classList.remove('is-dragging'); }
    if (!persist) { return; }
    if (moved && e) {
      commit(side, widthFromPointer(side, e.clientX), false);
    }
    var now = Date.now();
    if (!moved && lastClick && lastClick.side === side && (now - lastClick.at) < DBLCLICK_MS) {
      lastClick = null;
      reset(side);
      return;
    }
    if (!moved) {
      lastClick = { side: side, at: now };
    } else {
      lastClick = null;
    }
    storageSet(keyFor(side), shown[side]);
    preferred[side] = shown[side];
  }

  function onKeyDown(side, e) {
    if (isNarrow()) { return; }
    if (e.altKey || e.ctrlKey || e.metaKey) { return; }
    var key = e.key;
    var step = e.shiftKey ? SHIFT_STEP : STEP;
    var current = shown[side];
    var max = maxFor(otherShown(side));
    var handled = true;
    var next = current;
    if (key === 'ArrowRight' || key === 'Right') {
      // Right widens the left sidebar and narrows the right one.
      next = side === 'left' ? current + step : current - step;
    } else if (key === 'ArrowLeft' || key === 'Left') {
      next = side === 'left' ? current - step : current + step;
    } else if (key === 'Home') {
      next = MIN_WIDTH;
    } else if (key === 'End') {
      next = max;
    } else if (key === 'Enter') {
      e.preventDefault();
      reset(side);
      return;
    } else {
      handled = false;
    }
    if (!handled) { return; }
    e.preventDefault();
    commit(side, next, true);
  }

  function bind(side) {
    var handle = handleFor(side);
    if (!handle) { return; }
    handle.addEventListener('pointerdown', function (e) { onPointerDown(side, e); });
    handle.addEventListener('pointermove', onPointerMove);
    handle.addEventListener('pointerup', function (e) { finishDrag(e, true); });
    handle.addEventListener('pointercancel', function (e) { finishDrag(e, true); });
    handle.addEventListener('dblclick', function (e) {
      if (e.preventDefault) { e.preventDefault(); }
      lastClick = null;
      reset(side);
    });
    handle.addEventListener('keydown', function (e) { onKeyDown(side, e); });
  }

  var left0 = storedWidth(LEFT_KEY);
  var right0 = storedWidth(RIGHT_KEY);
  preferred.left = left0 == null ? DEFAULT_WIDTH : left0;
  preferred.right = right0 == null ? DEFAULT_WIDTH : right0;
  shown.left = preferred.left;
  shown.right = preferred.right;

  bind('left');
  bind('right');
  relayout();

  window.addEventListener('resize', relayout);
  if (window.MutationObserver) {
    var observer = new MutationObserver(relayout);
    observer.observe(document.body, { attributes: true, attributeFilter: ['class'] });
  }
})();
