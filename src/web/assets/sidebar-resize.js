'use strict';
// SPEC-sidebar-resize FR-970..FR-974, UX-970, UX-971: drag handles for the
// docked Sessions (left) and Files (right) sidebars. Sets
// --left-sidebar-width / --right-sidebar-width on <html>. Open/close stays
// in session.js and docs.js. At 900px and below this script does not write.
// Viewport width for the max is documentElement.clientWidth (scrollbar gutter
// excluded). A click with |dx| <= 3px does not write storage or preferred.

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
  var CLICK_SLOP = 3;

  var preferred = { left: DEFAULT_WIDTH, right: DEFAULT_WIDTH };
  var shown = { left: DEFAULT_WIDTH, right: DEFAULT_WIDTH };
  var drag = null;
  var lastClick = null;
  var windowDrag = false;
  var instantToken = 0;

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
    return window.matchMedia('(max-width: ' + NARROW_MAX + 'px)').matches;
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

  function sidebarEl(side) {
    return document.getElementById(side === 'left' ? 'sessions-sidebar' : 'sidebar');
  }

  function viewportWidth() {
    return document.documentElement.clientWidth;
  }

  // max = max(180, min(floor(viewport * 50%), viewport - otherOpenWidth - 360))
  function maxFor(otherWidth) {
    var vw = viewportWidth();
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

  // Drop the dock/width transition for a programmatic change. Open/close
  // still animates because this class is not applied for that slide.
  // Removed after two animation frames so a later open/close can transition.
  // Without requestAnimationFrame, drop the class after 50ms so it cannot stick.
  function beginInstant() {
    instantToken += 1;
    var token = instantToken;
    document.body.classList.add('sidebar-resize-instant');
    function clearInstant() {
      if (token !== instantToken) { return; }
      document.body.classList.remove('sidebar-resize-instant');
    }
    var raf = window.requestAnimationFrame;
    if (!raf) {
      window.setTimeout(clearInstant, 50);
      return;
    }
    raf(function () {
      raf(function () {
        clearInstant();
      });
    });
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
  // Does not write storage. instantIfChanged adds the instant class only
  // when a displayed width changes, so body padding does not lag that width
  // and an unchanged open/close slide is left alone.
  function relayout(instantIfChanged) {
    if (isNarrow()) { return; }
    if (drag) { return; }
    var prevLeft = shown.left;
    var prevRight = shown.right;
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
    if (instantIfChanged && (left !== prevLeft || right !== prevRight)) {
      beginInstant();
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

  // Left sidebar is docked at x = 0, so its width is clientX.
  // Right width is the sidebar's right edge minus clientX (not innerWidth,
  // so a classic scrollbar does not shift the handle).
  function widthFromPointer(side, clientX) {
    if (side === 'left') { return Math.round(clientX); }
    var el = sidebarEl(side);
    var rightEdge = el ? el.getBoundingClientRect().right : viewportWidth();
    return Math.round(rightEdge - clientX);
  }

  function addWindowDragListeners() {
    if (windowDrag) { return; }
    windowDrag = true;
    window.addEventListener('pointermove', onPointerMove);
    window.addEventListener('pointerup', onPointerUp);
    window.addEventListener('pointercancel', abortDrag);
  }

  function removeWindowDragListeners() {
    if (!windowDrag) { return; }
    windowDrag = false;
    window.removeEventListener('pointermove', onPointerMove);
    window.removeEventListener('pointerup', onPointerUp);
    window.removeEventListener('pointercancel', abortDrag);
  }

  function clearDragChrome(side) {
    removeWindowDragListeners();
    drag = null;
    document.body.classList.remove('sidebar-resizing');
    var handle = handleFor(side);
    if (handle) { handle.classList.remove('is-dragging'); }
  }

  function onPointerDown(side, e) {
    // A missed pointerup leaves drag set and the handle dead. Abort it first.
    if (drag) { abortDrag(); }
    if (typeof e.button === 'number' && e.button !== 0) { return; }
    if (isNarrow()) { return; }
    if (e.preventDefault) { e.preventDefault(); }
    var pointerWidth = widthFromPointer(side, e.clientX);
    drag = {
      side: side,
      pointerId: e.pointerId,
      startX: e.clientX,
      startWidth: shown[side],
      startPreferred: preferred[side],
      offset: shown[side] - pointerWidth,
      moved: false
    };
    document.body.classList.add('sidebar-resizing');
    var handle = handleFor(side);
    if (handle) { handle.classList.add('is-dragging'); }
    var captured = false;
    if (handle && handle.setPointerCapture) {
      try {
        handle.setPointerCapture(e.pointerId);
        captured = true;
      } catch (err) {
        captured = false;
      }
    }
    // Capture failed: follow the pointer on window until the drag ends,
    // including pointercancel so a cancel still aborts.
    if (!captured) { addWindowDragListeners(); }
  }

  function onPointerMove(e) {
    if (!drag || e.pointerId !== drag.pointerId) { return; }
    if (Math.abs(e.clientX - drag.startX) > CLICK_SLOP) { drag.moved = true; }
    // Sub-slop jitter stays a click: do not change width or preferred.
    if (!drag.moved) { return; }
    commit(drag.side, widthFromPointer(drag.side, e.clientX) + drag.offset, false);
  }

  function onPointerUp(e) {
    finishDrag(e);
  }

  // pointercancel / lostpointercapture: ignore coordinates, revert, do not persist.
  function abortDrag(e) {
    if (!drag) { return; }
    if (e && e.pointerId !== drag.pointerId) { return; }
    var side = drag.side;
    var startWidth = drag.startWidth;
    var startPreferred = drag.startPreferred;
    clearDragChrome(side);
    preferred[side] = startPreferred;
    shown[side] = startWidth;
    beginInstant();
    relayout();
  }

  function finishDrag(e) {
    if (!drag) { return; }
    if (e && e.pointerId !== drag.pointerId) { return; }
    var side = drag.side;
    var moved = drag.moved;
    var startWidth = drag.startWidth;
    var offset = drag.offset;
    clearDragChrome(side);
    if (!moved) {
      // A click must not overwrite a wider stored preference with the
      // clamped display width, and must not store the default.
      shown[side] = startWidth;
      var now = Date.now();
      if (lastClick && lastClick.side === side && (now - lastClick.at) < DBLCLICK_MS) {
        lastClick = null;
        beginInstant();
        reset(side);
      } else {
        lastClick = { side: side, at: now };
      }
      beginInstant();
      relayout();
      return;
    }
    lastClick = null;
    if (e) {
      commit(side, widthFromPointer(side, e.clientX) + offset, false);
    }
    storageSet(keyFor(side), shown[side]);
    preferred[side] = shown[side];
    beginInstant();
    relayout();
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
      beginInstant();
      reset(side);
      return;
    } else {
      handled = false;
    }
    if (!handled) { return; }
    e.preventDefault();
    beginInstant();
    commit(side, next, true);
  }

  function bind(side) {
    var handle = handleFor(side);
    if (!handle) { return; }
    handle.addEventListener('pointerdown', function (e) { onPointerDown(side, e); });
    handle.addEventListener('pointermove', onPointerMove);
    handle.addEventListener('pointerup', onPointerUp);
    handle.addEventListener('pointercancel', abortDrag);
    handle.addEventListener('lostpointercapture', abortDrag);
    handle.addEventListener('dblclick', function (e) {
      if (e.preventDefault) { e.preventDefault(); }
      lastClick = null;
      beginInstant();
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
  beginInstant();
  relayout();

  window.addEventListener('resize', function () {
    relayout(true);
  });
  if (window.MutationObserver) {
    var observer = new MutationObserver(function () { relayout(true); });
    observer.observe(document.body, { attributes: true, attributeFilter: ['class'] });
  }
})();
