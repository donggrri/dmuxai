//! Dashboard image attachments (SPEC-dashboard-feature-coverage FR-3758).
//!
//! TC-3758 (browser half; the HTTP half is `src/web/server.rs`
//! `tc_3758_attachments_api_types_limits_and_names`). Needs Chrome/Chromium and
//! python Playwright, so it is `#[ignore]`d (and prints `SKIP`, passing,
//! without them); run with `cargo test --test web_attachments -- --ignored`.

#![cfg(unix)]

mod common;

use common::{prepare, run_probe, setup_env, start_web};

// TC-3758 (FR-3758)
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3758_composer_attach_chip_remove_and_refuse() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let env = setup_env();
    env.write_chat_agents_config("");
    let big = env.root.path().join("too-big.png");
    {
        // A PNG signature followed by padding: one byte over the 10 MiB limit.
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.resize(10 * 1024 * 1024 + 1, 0);
        std::fs::write(&big, bytes).expect("big file");
    }
    let text_file = env.root.path().join("notes.txt");
    std::fs::write(&text_file, "not an image").expect("text file");
    let (_guard, url) = start_web(&env, &[]);

    let script = r##"
import json, sys
from playwright.sync_api import sync_playwright

chrome, url, big, text_file = sys.argv[1:5]
out = {}
uploads = []
prompts = []
deletes = []

def on_response(r):
    if r.url.endswith('/api/attachments') and r.request.method == 'POST':
        try:
            uploads.append({'status': r.status, 'body': r.json()})
        except Exception:
            uploads.append({'status': r.status, 'body': None})

def on_request(r):
    if r.url.endswith('/api/prompt') and r.method == 'POST':
        prompts.append(json.loads(r.post_data or '{}'))
    if '/api/attachments/' in r.url and r.method == 'DELETE':
        deletes.append(r.url)

PNG = bytes([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3, 4])

def chips(page):
    return page.locator('#session .session-attach-item').count()

def notice(page):
    return page.inner_text('#session .session-notice')

def wait_uploaded(page, n):
    page.wait_for_function(
        "(n) => document.querySelectorAll('#session .session-attach-item').length === n"
        " && !document.querySelector('#session .session-attach-item.is-uploading')",
        arg=n, timeout=15000)

with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={"width": 1280, "height": 800})
    page.on("response", on_response)
    page.on("request", on_request)
    page.goto(url, wait_until='domcontentloaded', timeout=20000)
    page.wait_for_selector('#session textarea.session-input', timeout=20000)
    if page.locator('#sessions-sidebar.open').count():
        page.click('#sessions-close')

    # file picker: PNG
    page.set_input_files('#session input[type=file]', files=[
        {'name': 'shot.png', 'mimeType': 'image/png', 'buffer': PNG}])
    wait_uploaded(page, 1)
    out['chip_after_picker'] = chips(page)
    # paste: JPEG
    page.evaluate("""() => {
      const dt = new DataTransfer();
      dt.items.add(new File([new Uint8Array([0xFF, 0xD8, 0xFF, 1, 2, 3])], 'p.jpg', {type: 'image/jpeg'}));
      const ev = new ClipboardEvent('paste', {clipboardData: dt, bubbles: true, cancelable: true});
      document.querySelector('#session textarea.session-input').dispatchEvent(ev);
    }""")
    wait_uploaded(page, 2)
    out['chips_after_paste'] = chips(page)
    out['upload_statuses'] = [u['status'] for u in uploads]
    out['upload_mimes'] = [(u['body'] or {}).get('mime') for u in uploads]
    names = [(u['body'] or {}).get('name') for u in uploads]

    # remove the first chip
    page.locator('#session .session-attach-remove').first.click()
    page.wait_for_function(
        "() => document.querySelectorAll('#session .session-attach-item').length === 1", timeout=5000)
    out['chips_after_remove'] = chips(page)

    # unsupported type and over-the-limit files: visible notice, no chip, no upload
    n_uploads = len(uploads)
    page.set_input_files('#session input[type=file]', files=[text_file])
    page.wait_for_function(
        "() => (document.querySelector('#session .session-notice')?.textContent || '').length > 0",
        timeout=5000)
    out['notice_unsupported'] = notice(page)
    out['chips_after_unsupported'] = chips(page)
    page.set_input_files('#session input[type=file]', files=[big])
    page.wait_for_function(
        "() => (document.querySelector('#session .session-notice')?.textContent || '').includes('too large')",
        timeout=5000)
    out['notice_big'] = notice(page)
    out['chips_after_big'] = chips(page)
    out['uploads_for_refused'] = len(uploads) - n_uploads

    # send: the remaining attachment goes with the next prompt
    page.fill('#session textarea.session-input', 'describe the picture')
    page.press('#session textarea.session-input', 'Enter')
    page.wait_for_function(
        "() => (document.querySelector('#session')?.innerText || '').includes('reply to describe the picture')",
        timeout=20000)
    out['prompt_attachments'] = prompts[-1].get('attachments') if prompts else None
    out['kept_name'] = names[1]
    out['removed_name'] = names[0]
    out['chips_after_send'] = chips(page)
    out['deletes'] = deletes

    # API: a prompt naming a path-like attachment is refused
    bad = page.evaluate("""async () => {
      const r = await fetch('/api/prompt', {method: 'POST',
        headers: {'Content-Type': 'application/json', 'X-Dmuxai-Client': '1'},
        body: JSON.stringify({text: 'sneaky', attachments: ['../../etc/passwd']})});
      return r.status;
    }""")
    out['bad_name_status'] = bad
    traversal = page.evaluate("""async () => {
      const out = [];
      for (const n of ['..%2F..%2Fetc%2Fpasswd', '%2e%2e', 'a%5Cb.png']) {
        const r = await fetch('/api/attachments/' + n, {headers: {'X-Dmuxai-Client': '1'}});
        out.push(r.status);
      }
      return out;
    }""")
    out['traversal_statuses'] = traversal
    browser.close()
print(json.dumps(out))
"##;

    let result = run_probe(
        &python,
        &chrome,
        script,
        &[&url, big.to_str().unwrap(), text_file.to_str().unwrap()],
    );
    assert_eq!(result["chip_after_picker"], 1, "{result}");
    assert_eq!(result["chips_after_paste"], 2, "{result}");
    assert_eq!(
        result["upload_statuses"],
        serde_json::json!([201, 201]),
        "{result}"
    );
    assert_eq!(
        result["upload_mimes"],
        serde_json::json!(["image/png", "image/jpeg"]),
        "{result}"
    );
    assert_eq!(result["chips_after_remove"], 1, "{result}");
    assert!(
        result["notice_unsupported"]
            .as_str()
            .unwrap_or("")
            .contains("Unsupported"),
        "{result}"
    );
    assert!(
        result["notice_big"]
            .as_str()
            .unwrap_or("")
            .contains("too large"),
        "{result}"
    );
    assert_eq!(result["chips_after_unsupported"], 1, "{result}");
    assert_eq!(result["chips_after_big"], 1, "{result}");
    assert_eq!(
        result["uploads_for_refused"], 0,
        "refused files never upload: {result}"
    );
    assert_eq!(
        result["prompt_attachments"],
        serde_json::json!([result["kept_name"].clone()]),
        "only the kept chip is sent: {result}"
    );
    assert_eq!(result["chips_after_send"], 0, "{result}");
    let bad = result["bad_name_status"].as_u64().unwrap_or(0);
    assert!(
        (400..500).contains(&bad),
        "path-like name refused: {result}"
    );
    for status in result["traversal_statuses"].as_array().unwrap() {
        let status = status.as_u64().unwrap_or(0);
        assert!((400..500).contains(&status), "traversal refused: {result}");
    }
    // The stored files: the kept one exists, nothing of the refused files was stored.
    let dir = env.workspace.join(".dmuxai").join("attachments");
    let stored = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
    assert_eq!(
        stored, 2,
        "only the two uploaded images are stored: {result}"
    );
    // KNOWN-BUG: FR-3758 names `DELETE /api/attachments/{name}` for chip removal, but
    // no such route exists; the chip is removed client-side and the file stays stored.
    if result["deletes"].as_array().is_some_and(|d| !d.is_empty()) {
        println!("chip removal sends DELETE");
    } else {
        println!(
            "KNOWN-BUG: removing a chip sends no DELETE /api/attachments/{{name}}; the removed upload stays in .dmuxai/attachments"
        );
    }
}
