//! TC-971 and TC-972 (SPEC-sidebar-resize): docked sidebar resize handles.
//! Needs Chrome/Chromium and python Playwright, so the tests are `#[ignore]`d.
//! Run with `cargo test --test web_sidebar_resize -- --ignored`.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn find_chrome() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("DMUXAI_CHROME") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let names = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome.exe",
        "msedge.exe",
    ];
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn prepare() -> Option<(PathBuf, &'static str)> {
    let Some(chrome) = find_chrome() else {
        println!("SKIP: no Chrome/Chromium found (set DMUXAI_CHROME)");
        return None;
    };
    let python = if cfg!(windows) { "python" } else { "python3" };
    let probe = Command::new(python)
        .args(["-c", "import playwright"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !probe.map(|status| status.success()).unwrap_or(false) {
        println!("SKIP: python playwright not installed");
        return None;
    }
    Some((chrome, python))
}

struct Dashboard {
    _guard: ChildGuard,
    url: String,
    _workspace: tempfile::TempDir,
    _home: tempfile::TempDir,
}

fn start_dashboard() -> Dashboard {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let home = tempfile::tempdir().expect("temp home");
    std::fs::write(workspace.path().join("README.md"), "# Sidebar resize\n").expect("write doc");

    let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["serve", "--port", "0"])
        .current_dir(workspace.path())
        .env("HOME", home.path())
        .env_remove("DMUXAI_WORKSPACE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn dmuxai serve");
    let stdout = child.stdout.take().expect("stdout is piped");
    let guard = ChildGuard(Some(child));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("dashboard URL line");
    let url = line
        .trim()
        .strip_prefix("dmuxai dashboard: ")
        .unwrap_or_else(|| panic!("unexpected first stdout line: {line:?}"))
        .to_owned();
    Dashboard {
        _guard: guard,
        url,
        _workspace: workspace,
        _home: home,
    }
}

fn run_probe(chrome: &Path, url: &str, script: &str) -> Value {
    let python = if cfg!(windows) { "python" } else { "python3" };
    let mut py_child = Command::new(python)
        .args(["-c", script])
        .arg(chrome)
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn playwright");
    let (tx_out, rx_out) = mpsc::channel();
    let mut py_stdout = py_child.stdout.take().expect("py stdout");
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = py_stdout.read_to_end(&mut out);
        let _ = tx_out.send(out);
    });
    let out_bytes = match rx_out.recv_timeout(Duration::from_secs(120)) {
        Ok(b) => {
            let _ = py_child.wait();
            b
        }
        Err(_) => {
            let _ = py_child.kill();
            let _ = py_child.wait();
            panic!("playwright execution timed out after 120s");
        }
    };
    let text = String::from_utf8_lossy(&out_bytes);
    let json_line = text
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON result from playwright:\n{text}"));
    serde_json::from_str(json_line).expect("json result")
}

fn num(v: &Value, key: &str) -> f64 {
    v.get(key)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("{key} missing in {v}"))
}

fn approx(label: &str, actual: f64, expected: f64, tol: f64) {
    assert!(
        (actual - expected).abs() <= tol,
        "{label}: {actual} not within {tol} of {expected}"
    );
}

fn max_width(inner: f64, other: f64) -> f64 {
    let cap = (inner * 0.5).floor().min(inner - other - 360.0);
    cap.max(180.0)
}

fn clamp_w(value: f64, max: f64) -> f64 {
    value.round().clamp(180.0, max)
}

/// Mirrors sidebar-resize.js `relayout` for a known preference.
fn settle(
    inner: f64,
    left_pref: f64,
    right_pref: f64,
    left_open: bool,
    right_open: bool,
) -> (f64, f64) {
    let mut left = clamp_w(left_pref, max_width(inner, 0.0));
    let mut right = clamp_w(right_pref, max_width(inner, 0.0));
    if left_open && right_open {
        for _ in 0..3 {
            left = clamp_w(left_pref, max_width(inner, right));
            right = clamp_w(right_pref, max_width(inner, left));
        }
    } else if left_open {
        left = clamp_w(left_pref, max_width(inner, 0.0));
    } else if right_open {
        right = clamp_w(right_pref, max_width(inner, 0.0));
    }
    (left, right)
}

fn assert_prompt(label: &str, step: &Value) {
    let iw = num(step, "innerWidth");
    let prompt = &step["prompt"];
    let p_left = num(prompt, "left");
    let p_right = num(prompt, "right");
    let p_width = num(prompt, "width");
    let center = p_left + p_width / 2.0;
    let left_open = step["leftOpen"].as_bool().expect("leftOpen");
    let right_open = step["rightOpen"].as_bool().expect("rightOpen");
    let left_dock = if left_open {
        num(&step["left"], "width")
    } else {
        0.0
    };
    let right_dock = if right_open {
        num(&step["right"], "width")
    } else {
        0.0
    };
    let pad_l = num(step, "paddingLeft");
    let pad_r = num(step, "paddingRight");
    approx(&format!("{label} padding-left"), pad_l, left_dock, 1.5);
    approx(&format!("{label} padding-right"), pad_r, right_dock, 1.5);
    let content_center = (pad_l + (iw - pad_r)) / 2.0;
    approx(
        &format!("{label} prompt center"),
        center,
        content_center,
        2.0,
    );
    if left_open {
        let edge = num(&step["left"], "right");
        assert!(
            p_left + 0.5 >= edge,
            "{label}: prompt left {p_left} overlaps left sidebar right {edge}"
        );
    }
    if right_open {
        let edge = num(&step["right"], "left");
        assert!(
            p_right - 0.5 <= edge,
            "{label}: prompt right {p_right} overlaps right sidebar left {edge}"
        );
    }
}

fn assert_width(label: &str, step: &Value, side: &str, expected: f64) {
    let aria_key = if side == "left" {
        "leftAria"
    } else {
        "rightAria"
    };
    let rect_key = if side == "left" { "left" } else { "right" };
    let actual_aria = num(step, aria_key);
    let actual_rect = num(&step[rect_key], "width");
    approx(&format!("{label} {side} aria"), actual_aria, expected, 0.1);
    approx(&format!("{label} {side} width"), actual_rect, expected, 1.5);
}

fn storage_is(step: &Value, key: &str, expected: Option<&str>) {
    let v = &step[key];
    match expected {
        None => assert!(v.is_null(), "{key} should be cleared, got {v}"),
        Some(text) => assert_eq!(v.as_str(), Some(text), "{key} storage"),
    }
}

const PROBE_COMMON: &str = r##"
import sys, json
from playwright.sync_api import sync_playwright

SETTLE = 450

SNAPSHOT = """() => {
  function rect(sel) {
    var e = document.querySelector(sel);
    if (!e) return null;
    var r = e.getBoundingClientRect();
    return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height};
  }
  function hit(id, x, y) {
    var el = document.elementFromPoint(x, y);
    while (el) {
      if (el.id === id) return true;
      el = el.parentElement;
    }
    return false;
  }
  var lh = document.getElementById('sessions-resize');
  var rh = document.getElementById('sidebar-resize');
  var ls = document.getElementById('sessions-sidebar').getBoundingClientRect();
  var rs = document.getElementById('sidebar').getBoundingClientRect();
  var lhb = lh.getBoundingClientRect();
  var rhb = rh.getBoundingClientRect();
  var cs = getComputedStyle(document.body);
  var ly = lhb.top + Math.min(Math.max(lhb.height / 2, 8), Math.max(lhb.height - 4, 8));
  var ry = rhb.top + Math.min(Math.max(rhb.height / 2, 8), Math.max(rhb.height - 4, 8));
  var probe = document.createElement('div');
  probe.style.cssText = 'position:absolute;left:0;top:0;width:85vw;height:0;visibility:hidden;';
  document.body.appendChild(probe);
  var vw85 = probe.getBoundingClientRect().width;
  probe.remove();
  function stored(key) {
    try { return localStorage.getItem(key); } catch (e) { return null; }
  }
  return {
    innerWidth: window.innerWidth,
    paddingLeft: parseFloat(cs.paddingLeft) || 0,
    paddingRight: parseFloat(cs.paddingRight) || 0,
    leftOpen: document.body.classList.contains('left-sidebar-open'),
    rightOpen: document.body.classList.contains('sidebar-open'),
    resizing: document.body.classList.contains('sidebar-resizing'),
    prompt: rect('.session-prompt'),
    left: rect('#sessions-sidebar'),
    right: rect('#sidebar'),
    leftAria: Number(lh.getAttribute('aria-valuenow')),
    rightAria: Number(rh.getAttribute('aria-valuenow')),
    leftMax: Number(lh.getAttribute('aria-valuemax')),
    rightMax: Number(rh.getAttribute('aria-valuemax')),
    leftMin: Number(lh.getAttribute('aria-valuemin')),
    rightMin: Number(rh.getAttribute('aria-valuemin')),
    leftRole: lh.getAttribute('role'),
    rightRole: rh.getAttribute('role'),
    leftStorage: stored('dmuxai.sidebar.width.left'),
    rightStorage: stored('dmuxai.sidebar.width.right'),
    leftHandle: {
      width: lhb.width,
      center: lhb.left + lhb.width / 2,
      display: getComputedStyle(lh).display,
      cursor: getComputedStyle(lh).cursor
    },
    rightHandle: {
      width: rhb.width,
      center: rhb.left + rhb.width / 2,
      display: getComputedStyle(rh).display,
      cursor: getComputedStyle(rh).cursor
    },
    leftEdge: ls.right,
    rightEdge: rs.left,
    hitLeftInner: hit('sessions-resize', ls.right - 2, ly),
    hitLeftOuter: hit('sessions-resize', ls.right + 2, ly),
    hitRightInner: hit('sidebar-resize', rs.left + 2, ry),
    hitRightOuter: hit('sidebar-resize', rs.left - 2, ry),
    vw85: vw85
  };
}"""

def snap(pg, out, label, extra=None):
    data = pg.evaluate(SNAPSHOT)
    if extra:
        data.update(extra)
    out["steps"][label] = data
    return data

def ensure_left(pg, open_):
    is_open = pg.eval_on_selector("#sessions-sidebar", "e => e.classList.contains('open')")
    if bool(is_open) == bool(open_):
        return
    if open_:
        pg.click("#sessions-toggle")
        pg.wait_for_selector("#sessions-sidebar.open", timeout=8000)
    else:
        pg.click("#sessions-close")
        pg.wait_for_selector("#sessions-sidebar:not(.open)", timeout=8000)
    pg.wait_for_timeout(SETTLE)

def ensure_right(pg, open_):
    is_open = pg.eval_on_selector("#sidebar", "e => e.classList.contains('open')")
    if bool(is_open) == bool(open_):
        return
    pg.click("#sidebar-toggle-files")
    if open_:
        pg.wait_for_selector("#sidebar.open", timeout=8000)
    else:
        pg.wait_for_selector("#sidebar:not(.open)", timeout=8000)
    pg.wait_for_timeout(SETTLE)

def drag(pg, sel, x):
    box = pg.locator(sel).bounding_box()
    if not box:
        raise Exception("no bounding box for " + sel)
    y = box["y"] + box["height"] / 2
    start = box["x"] + box["width"] / 2
    pg.mouse.move(start, y)
    pg.mouse.down()
    pg.mouse.move((start + x) / 2.0, y, steps=4)
    resizing = pg.evaluate("() => document.body.classList.contains('sidebar-resizing')")
    pg.mouse.move(x, y, steps=8)
    pg.mouse.up()
    pg.wait_for_timeout(SETTLE)
    return resizing

def boot(pg, width, height):
    pg.set_viewport_size({"width": width, "height": height})
    pg.goto(sys.argv[2])
    pg.wait_for_selector("#session:not([hidden]) .session-prompt", timeout=15000)
    pg.wait_for_selector("#sessions-toggle", timeout=15000)
    pg.wait_for_selector("#sessions-resize", timeout=15000)
    pg.wait_for_timeout(SETTLE)
"##;

// TC-971: sidebar_resize_in_headless_chrome (FR-970, FR-972, FR-973, UX-970)
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn sidebar_resize_in_headless_chrome() {
    let Some((chrome, _python)) = prepare() else {
        return;
    };
    let dashboard = start_dashboard();
    let script = format!(
        "{PROBE_COMMON}\n{}",
        r##"
out = {"steps": {}}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page(viewport={"width": 1440, "height": 900}, bypass_csp=True)
    boot(pg, 1440, 900)
    ensure_left(pg, True)
    ensure_right(pg, True)
    snap(pg, out, "base")

    resizing = drag(pg, "#sessions-resize", 480)
    snap(pg, out, "drag_left", {"resizingDuring": resizing})
    iw = pg.evaluate("() => window.innerWidth")
    resizing = drag(pg, "#sidebar-resize", iw - 400)
    snap(pg, out, "drag_right", {"resizingDuring": resizing})

    pg.reload()
    pg.wait_for_selector("#session:not([hidden]) .session-prompt", timeout=15000)
    pg.wait_for_timeout(SETTLE)
    ensure_left(pg, True)
    ensure_right(pg, True)
    snap(pg, out, "reloaded")

    pg.focus("#sessions-resize")
    pg.keyboard.press("ArrowRight")
    pg.wait_for_timeout(SETTLE)
    snap(pg, out, "left_arrow_right")
    pg.keyboard.press("ArrowLeft")
    pg.wait_for_timeout(SETTLE)
    snap(pg, out, "left_arrow_left")

    pg.focus("#sidebar-resize")
    pg.keyboard.press("ArrowLeft")
    pg.wait_for_timeout(SETTLE)
    snap(pg, out, "right_arrow_left")
    pg.keyboard.press("ArrowRight")
    pg.wait_for_timeout(SETTLE)
    snap(pg, out, "right_arrow_right")

    pg.focus("#sessions-resize")
    pg.keyboard.press("Shift+ArrowRight")
    pg.wait_for_timeout(SETTLE)
    snap(pg, out, "shift_right")
    pg.keyboard.press("Home")
    pg.wait_for_timeout(SETTLE)
    snap(pg, out, "home")
    pg.keyboard.press("End")
    pg.wait_for_timeout(SETTLE)
    snap(pg, out, "end")
    pg.keyboard.press("Enter")
    pg.wait_for_timeout(SETTLE)
    snap(pg, out, "enter_reset")

    pg.dblclick("#sidebar-resize")
    pg.wait_for_timeout(SETTLE)
    snap(pg, out, "dbl_reset")

    print(json.dumps(out))
    b.close()
"##
    );
    let v = run_probe(&chrome, &dashboard.url, &script);
    let steps = &v["steps"];

    let base = &steps["base"];
    assert_eq!(base["leftRole"], "separator");
    assert_eq!(base["rightRole"], "separator");
    approx("left min", num(base, "leftMin"), 180.0, 0.1);
    approx("right min", num(base, "rightMin"), 180.0, 0.1);
    assert_width("base", base, "left", 320.0);
    assert_width("base", base, "right", 320.0);
    assert!(num(&base["leftHandle"], "width") >= 6.0, "left hit area");
    assert!(num(&base["rightHandle"], "width") >= 6.0, "right hit area");
    approx(
        "left handle center",
        num(&base["leftHandle"], "center"),
        num(base, "leftEdge"),
        2.0,
    );
    approx(
        "right handle center",
        num(&base["rightHandle"], "center"),
        num(base, "rightEdge"),
        2.0,
    );
    assert_eq!(base["leftHandle"]["cursor"], "col-resize");
    assert_eq!(base["rightHandle"]["cursor"], "col-resize");
    assert_eq!(base["leftHandle"]["display"], "block");
    assert_ne!(base["leftHandle"]["display"], "none");
    assert!(
        base["hitLeftInner"].as_bool().unwrap(),
        "left handle inner hit"
    );
    assert!(
        base["hitLeftOuter"].as_bool().unwrap(),
        "left handle outer hit"
    );
    assert!(
        base["hitRightInner"].as_bool().unwrap(),
        "right handle inner hit"
    );
    assert!(
        base["hitRightOuter"].as_bool().unwrap(),
        "right handle outer hit"
    );
    assert_prompt("base", base);
    storage_is(base, "leftStorage", None);
    storage_is(base, "rightStorage", None);

    let drag_left = &steps["drag_left"];
    assert_eq!(
        drag_left["resizingDuring"], true,
        "dragging sets sidebar-resizing"
    );
    assert_eq!(
        drag_left["resizing"], false,
        "class cleared after pointerup"
    );
    assert_width("drag_left", drag_left, "left", 480.0);
    assert_width("drag_left", drag_left, "right", 320.0);
    storage_is(drag_left, "leftStorage", Some("480"));
    assert_prompt("drag_left", drag_left);

    let drag_right = &steps["drag_right"];
    assert_eq!(drag_right["resizingDuring"], true);
    assert_width("drag_right", drag_right, "left", 480.0);
    assert_width("drag_right", drag_right, "right", 400.0);
    storage_is(drag_right, "rightStorage", Some("400"));
    assert_prompt("drag_right", drag_right);

    let reloaded = &steps["reloaded"];
    assert_width("reloaded", reloaded, "left", 480.0);
    assert_width("reloaded", reloaded, "right", 400.0);
    storage_is(reloaded, "leftStorage", Some("480"));
    storage_is(reloaded, "rightStorage", Some("400"));
    assert_prompt("reloaded", reloaded);

    assert_width("left ArrowRight", &steps["left_arrow_right"], "left", 496.0);
    assert_width("left ArrowLeft", &steps["left_arrow_left"], "left", 480.0);
    assert_width(
        "right ArrowLeft",
        &steps["right_arrow_left"],
        "right",
        416.0,
    );
    assert_width(
        "right ArrowRight",
        &steps["right_arrow_right"],
        "right",
        400.0,
    );
    assert_width("shift", &steps["shift_right"], "left", 544.0);

    let home = &steps["home"];
    assert_width("home", home, "left", 180.0);
    assert_prompt("home", home);

    let end = &steps["end"];
    let end_max = max_width(num(end, "innerWidth"), 400.0);
    assert_width("end", end, "left", end_max);
    approx("end aria max", num(end, "leftMax"), end_max, 0.1);
    assert_width("end right stays", end, "right", 400.0);
    assert_prompt("end", end);

    let enter = &steps["enter_reset"];
    assert_width("enter", enter, "left", 320.0);
    storage_is(enter, "leftStorage", None);
    assert_width("enter right stays", enter, "right", 400.0);

    let dbl = &steps["dbl_reset"];
    assert_width("dblclick", dbl, "right", 320.0);
    assert_width("dblclick left stays", dbl, "left", 320.0);
    storage_is(dbl, "rightStorage", None);
    storage_is(dbl, "leftStorage", None);
    assert_prompt("dblclick", dbl);
}

// TC-972: sidebar_resize_clamps_and_narrow_layout (FR-971, FR-972, FR-974)
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn sidebar_resize_clamps_and_narrow_layout() {
    let Some((chrome, _python)) = prepare() else {
        return;
    };
    let dashboard = start_dashboard();
    let script = format!(
        "{PROBE_COMMON}\n{}",
        r##"
out = {"steps": {}}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page(viewport={"width": 1440, "height": 900}, bypass_csp=True)
    boot(pg, 1440, 900)
    ensure_left(pg, True)
    ensure_right(pg, True)

    drag(pg, "#sessions-resize", 12)
    snap(pg, out, "min_left")
    iw = pg.evaluate("() => window.innerWidth")
    drag(pg, "#sidebar-resize", iw - 500)
    snap(pg, out, "right_500")
    drag(pg, "#sessions-resize", iw - 8)
    snap(pg, out, "left_max")
    drag(pg, "#sessions-resize", 420)
    snap(pg, out, "left_420")

    ensure_left(pg, False)
    snap(pg, out, "left_closed")
    ensure_left(pg, True)
    snap(pg, out, "left_reopened")
    ensure_right(pg, False)
    snap(pg, out, "right_closed")
    ensure_right(pg, True)
    snap(pg, out, "right_reopened")

    ensure_right(pg, False)
    drag(pg, "#sessions-resize", 700)
    snap(pg, out, "left_700")
    pg.set_viewport_size({"width": 1200, "height": 900})
    pg.wait_for_timeout(SETTLE)
    snap(pg, out, "resized_narrower")
    pg.set_viewport_size({"width": 1440, "height": 900})
    pg.wait_for_timeout(SETTLE)
    snap(pg, out, "resized_back")
    ensure_right(pg, True)
    snap(pg, out, "other_opened")

    pg.evaluate("""() => {
      localStorage.setItem('dmuxai.sidebar.width.left', '500');
      localStorage.setItem('dmuxai.sidebar.width.right', '500');
    }""")
    pg.set_viewport_size({"width": 800, "height": 900})
    pg.reload()
    pg.wait_for_selector("#session:not([hidden]) .session-prompt", timeout=15000)
    pg.wait_for_timeout(SETTLE)
    snap(pg, out, "narrow")
    ensure_left(pg, True)
    ensure_right(pg, True)
    snap(pg, out, "narrow_open")

    pg.set_viewport_size({"width": 1440, "height": 900})
    pg.wait_for_timeout(SETTLE)
    ensure_left(pg, True)
    ensure_right(pg, True)
    snap(pg, out, "wide_again")

    pg.evaluate("""() => {
      localStorage.setItem('dmuxai.sidebar.width.left', 'nope');
      localStorage.setItem('dmuxai.sidebar.width.right', '12');
    }""")
    pg.reload()
    pg.wait_for_selector("#session:not([hidden]) .session-prompt", timeout=15000)
    pg.wait_for_timeout(SETTLE)
    snap(pg, out, "bad_storage")

    print(json.dumps(out))
    b.close()
"##
    );
    let v = run_probe(&chrome, &dashboard.url, &script);
    let steps = &v["steps"];

    let min_left = &steps["min_left"];
    assert_width("min", min_left, "left", 180.0);
    assert_width("min right stays", min_left, "right", 320.0);
    assert_prompt("min", min_left);

    let right_500 = &steps["right_500"];
    assert_width("right 500", right_500, "right", 500.0);
    assert_width("right 500 left stays", right_500, "left", 180.0);
    storage_is(right_500, "rightStorage", Some("500"));
    assert_prompt("right 500", right_500);

    let left_max = &steps["left_max"];
    let iw = num(left_max, "innerWidth");
    let expected_max = max_width(iw, 500.0);
    assert!(
        expected_max < iw * 0.5,
        "fixture should bind on the other sidebar, not only the 50% cap: max {expected_max} iw {iw}"
    );
    assert_width("left max", left_max, "left", expected_max);
    assert_width("left max right stays", left_max, "right", 500.0);
    approx("left aria max", num(left_max, "leftMax"), expected_max, 0.1);
    assert_prompt("left max", left_max);

    let left_420 = &steps["left_420"];
    assert_width("420", left_420, "left", 420.0);
    assert_width("420 right", left_420, "right", 500.0);
    assert_prompt("420", left_420);

    let closed = &steps["left_closed"];
    assert!(!closed["leftOpen"].as_bool().unwrap());
    approx("closed left padding", num(closed, "paddingLeft"), 0.0, 1.0);
    assert!(
        num(&closed["left"], "right") <= 2.0,
        "closed left sidebar still on screen: {}",
        closed["left"]
    );
    assert_width("closed keeps right", closed, "right", 500.0);
    assert_prompt("left closed", closed);

    let reopened = &steps["left_reopened"];
    assert!(reopened["leftOpen"].as_bool().unwrap());
    assert_width("reopened left", reopened, "left", 420.0);
    assert_width("reopened right", reopened, "right", 500.0);
    assert_prompt("left reopened", reopened);

    let right_closed = &steps["right_closed"];
    assert!(!right_closed["rightOpen"].as_bool().unwrap());
    approx(
        "closed right padding",
        num(right_closed, "paddingRight"),
        0.0,
        1.0,
    );
    assert!(
        num(&right_closed["right"], "left") >= num(right_closed, "innerWidth") - 2.0,
        "closed right sidebar still on screen: {}",
        right_closed["right"]
    );
    assert_width("right closed keeps left", right_closed, "left", 420.0);
    assert_prompt("right closed", right_closed);

    let right_reopened = &steps["right_reopened"];
    assert_width("right reopened", right_reopened, "right", 500.0);
    assert_width("right reopened left", right_reopened, "left", 420.0);
    assert_prompt("right reopened", right_reopened);

    let left_700 = &steps["left_700"];
    assert!(!left_700["rightOpen"].as_bool().unwrap());
    assert_width("700", left_700, "left", 700.0);
    storage_is(left_700, "leftStorage", Some("700"));
    assert_prompt("700", left_700);

    let resized = &steps["resized_narrower"];
    let resized_iw = num(resized, "innerWidth");
    let (expect_left, _) = settle(resized_iw, 700.0, 500.0, true, false);
    assert_width("resized", resized, "left", expect_left);
    assert!(
        expect_left < 700.0,
        "narrower viewport should clamp 700, got {expect_left} at {resized_iw}"
    );
    storage_is(resized, "leftStorage", Some("700"));
    storage_is(resized, "rightStorage", Some("500"));
    assert_prompt("resized", resized);

    let back = &steps["resized_back"];
    let back_iw = num(back, "innerWidth");
    let (expect_back, _) = settle(back_iw, 700.0, 500.0, true, false);
    assert_width("resized back", back, "left", expect_back);
    storage_is(back, "leftStorage", Some("700"));
    assert_prompt("resized back", back);

    let opened = &steps["other_opened"];
    let opened_iw = num(opened, "innerWidth");
    let (expect_l, expect_r) = settle(opened_iw, 700.0, 500.0, true, true);
    assert_width("other opened left", opened, "left", expect_l);
    assert_width("other opened right", opened, "right", expect_r);
    assert!(
        expect_l < 700.0,
        "opening the other sidebar should reclamp 700, got {expect_l}"
    );
    storage_is(opened, "leftStorage", Some("700"));
    storage_is(opened, "rightStorage", Some("500"));
    assert_prompt("other opened", opened);

    let narrow = &steps["narrow"];
    assert_eq!(narrow["leftHandle"]["display"], "none");
    assert_eq!(narrow["rightHandle"]["display"], "none");
    let drawer = 320.0_f64.min(num(narrow, "vw85"));
    approx(
        "narrow left drawer",
        num(&narrow["left"], "width"),
        drawer,
        1.5,
    );
    approx(
        "narrow right drawer",
        num(&narrow["right"], "width"),
        drawer,
        1.5,
    );
    assert!(
        (num(&narrow["left"], "width") - 500.0).abs() > 50.0,
        "stored 500 must not size the narrow drawer"
    );
    storage_is(narrow, "leftStorage", Some("500"));
    storage_is(narrow, "rightStorage", Some("500"));
    let n_iw = num(narrow, "innerWidth");
    let n_prompt = &narrow["prompt"];
    let n_center = num(n_prompt, "left") + num(n_prompt, "width") / 2.0;
    approx("narrow prompt center", n_center, n_iw / 2.0, 2.0);
    let min_prompt = (n_iw - 20.0).min(320.0);
    assert!(
        num(n_prompt, "width") + 0.5 >= min_prompt,
        "narrow prompt width {} < {min_prompt}",
        num(n_prompt, "width")
    );

    let narrow_open = &steps["narrow_open"];
    assert_eq!(narrow_open["leftHandle"]["display"], "none");
    assert_eq!(narrow_open["rightHandle"]["display"], "none");
    let drawer_open = 320.0_f64.min(num(narrow_open, "vw85"));
    approx(
        "narrow open left",
        num(&narrow_open["left"], "width"),
        drawer_open,
        1.5,
    );
    approx(
        "narrow open right",
        num(&narrow_open["right"], "width"),
        drawer_open,
        1.5,
    );
    storage_is(narrow_open, "leftStorage", Some("500"));
    storage_is(narrow_open, "rightStorage", Some("500"));
    approx(
        "narrow open padL",
        num(narrow_open, "paddingLeft"),
        0.0,
        1.0,
    );
    approx(
        "narrow open padR",
        num(narrow_open, "paddingRight"),
        0.0,
        1.0,
    );
    let o_prompt = &narrow_open["prompt"];
    let o_center = num(o_prompt, "left") + num(o_prompt, "width") / 2.0;
    approx(
        "narrow open prompt",
        o_center,
        num(narrow_open, "innerWidth") / 2.0,
        2.0,
    );

    let wide = &steps["wide_again"];
    let wide_iw = num(wide, "innerWidth");
    let (wide_l, wide_r) = settle(wide_iw, 500.0, 500.0, true, true);
    assert_width("wide again left", wide, "left", wide_l);
    assert_width("wide again right", wide, "right", wide_r);
    storage_is(wide, "leftStorage", Some("500"));
    storage_is(wide, "rightStorage", Some("500"));
    assert_ne!(wide["leftHandle"]["display"], "none");
    assert_prompt("wide again", wide);

    let bad = &steps["bad_storage"];
    let bad_iw = num(bad, "innerWidth");
    let left_open = bad["leftOpen"].as_bool().unwrap();
    let right_open = bad["rightOpen"].as_bool().unwrap();
    let (bad_l, bad_r) = settle(bad_iw, 320.0, 12.0, left_open, right_open);
    assert_width("non-numeric left", bad, "left", bad_l);
    assert_width("below-min right", bad, "right", bad_r);
    assert!(
        (bad_l - 320.0).abs() < 1.0,
        "non-numeric left should fall back to 320, got {bad_l}"
    );
    assert!(
        (bad_r - 180.0).abs() < 1.0,
        "below-min right should clamp to 180, got {bad_r}"
    );
}
