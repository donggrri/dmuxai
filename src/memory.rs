//! Obsidian memory (SPEC-obsidian-memory, FR-720..FR-731).
//!
//! Port of cursor-atomic-workflow `scripts/memory.mjs`: one-line dated
//! entries in markdown files under `<vault>/<dir>`, a bounded `<memory>`
//! context block, direct human saves and an approval queue for agent
//! proposals. Pure filesystem; no network, no SQLite.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

/// Whole `<memory>` body budget in chars (FR-721).
pub const CONTEXT_BUDGET: usize = 3000;
/// Per-section budget in chars (FR-721).
pub const SECTION_BUDGET: usize = 900;
/// Max chars of one entry text (FR-723).
pub const MAX_ENTRY_CHARS: usize = 200;
/// Default memory directory inside the vault (FR-720).
pub const DEFAULT_DIR: &str = "cursor-workflow/memory";

static ENTRY_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"^- \[\d{4}-\d{2}-\d{2}\] ").unwrap());

static NAME_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9_-]{0,47}$").unwrap());

static TARGET_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"^(projects|decisions|gotchas)/([a-z0-9][a-z0-9_-]{0,47})\.md$").unwrap()
});

static SECRET_PATTERNS: std::sync::LazyLock<Vec<Regex>> = std::sync::LazyLock::new(|| {
    vec![
        RegexBuilder::new(r"password|passwd|비밀번호")
            .case_insensitive(true)
            .build()
            .unwrap(),
        RegexBuilder::new(r"secret")
            .case_insensitive(true)
            .build()
            .unwrap(),
        RegexBuilder::new(r"\btoken\b")
            .case_insensitive(true)
            .build()
            .unwrap(),
        RegexBuilder::new(r"api[_ -]?key")
            .case_insensitive(true)
            .build()
            .unwrap(),
        RegexBuilder::new(r"bearer\s+\S+")
            .case_insensitive(true)
            .build()
            .unwrap(),
        Regex::new(r"\bsk-[A-Za-z0-9]{8,}").unwrap(),
        Regex::new(r"\bghp_[A-Za-z0-9]{10,}").unwrap(),
        Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").unwrap(),
    ]
});

static QUEUE_MUTEX: Mutex<()> = Mutex::new(());
/// Serializes read-modify-write of memory markdown files within a process.
static FILE_MUTEX: Mutex<()> = Mutex::new(());

/// Resolved `[memory]` settings (FR-720). `root` is `<vault>/<dir>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConfig {
    pub root: PathBuf,
    /// The configured `dir` (relative, for display; never the absolute path).
    pub dir: String,
    /// Explicit `[memory] project`, if valid.
    pub project: Option<String>,
    /// `[memory] inject` (default true).
    pub inject: bool,
}

/// True if `name` matches `[a-z0-9][a-z0-9_-]{0,47}` (target and project names).
pub fn is_valid_name(name: &str) -> bool {
    NAME_RE.is_match(name)
}

/// FR-720 / NFR-731: `<vault>/<dir>` with `dir` normalized to `/` separators.
/// `vault` must be an existing directory; `dir` (default [`DEFAULT_DIR`]) must
/// be relative with only normal components (no `..`, root or drive prefix).
/// Returns `(root, normalized_dir)` or a message naming the bad value.
pub fn resolve_root(vault: &Path, dir: Option<&str>) -> Result<(PathBuf, String), String> {
    if !vault.is_dir() {
        return Err(format!(
            "vault \"{}\" is not an existing directory",
            vault.display()
        ));
    }
    let dir = dir
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .unwrap_or(DEFAULT_DIR)
        .replace('\\', "/");
    let dir_path = Path::new(&dir);
    let bad = dir_path.is_absolute()
        || dir.starts_with('/')
        || dir_path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)));
    if bad {
        return Err(format!(
            "dir \"{dir}\" must be a relative path inside the vault without '..'"
        ));
    }
    let mut root = vault.to_path_buf();
    for part in dir.split('/').filter(|p| !p.is_empty()) {
        root.push(part);
    }
    Ok((root, dir))
}

/// Kind of a memory target file (FR-723).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    Human,
    Projects,
    Decisions,
    Gotchas,
}

/// A validated target such as `projects/dmuxai.md` (FR-723).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub kind: TargetKind,
    /// `human` for `human.md`, else the `<name>` part.
    pub name: String,
}

impl Target {
    /// Parse `human.md` or `(projects|decisions|gotchas)/<name>.md`.
    pub fn parse(target: &str) -> Option<Target> {
        if target.contains('\\') || target.contains("..") {
            return None;
        }
        if target == "human.md" {
            return Some(Target {
                kind: TargetKind::Human,
                name: "human".to_string(),
            });
        }
        let caps = TARGET_RE.captures(target)?;
        let kind_str = caps.get(1)?.as_str();
        let name_str = caps.get(2)?.as_str();
        let kind = match kind_str {
            "projects" => TargetKind::Projects,
            "decisions" => TargetKind::Decisions,
            "gotchas" => TargetKind::Gotchas,
            _ => return None,
        };
        Some(Target {
            kind,
            name: name_str.to_string(),
        })
    }

    /// `projects/<project>.md`.
    pub fn project(project: &str) -> Option<Target> {
        Self::parse(&format!("projects/{project}.md"))
    }

    /// Relative path string, e.g. `projects/dmuxai.md` or `human.md`.
    pub fn as_rel(&self) -> String {
        match self.kind {
            TargetKind::Human => "human.md".to_string(),
            TargetKind::Projects => format!("projects/{}.md", self.name),
            TargetKind::Decisions => format!("decisions/{}.md", self.name),
            TargetKind::Gotchas => format!("gotchas/{}.md", self.name),
        }
    }
}

/// Why a text/target was refused (FR-723). `Display` gives the reason text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    Empty,
    MultiLine,
    TooLong,
    Secret,
    InvalidTarget,
    InvalidSource,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Rejection::Empty => "empty",
            Rejection::MultiLine => "multi-line",
            Rejection::TooLong => "too long (max 200 chars)",
            Rejection::Secret => "looks like a secret (not saved)",
            Rejection::InvalidTarget => {
                "invalid target (human.md | projects|decisions|gotchas/<name>.md)"
            }
            Rejection::InvalidSource => "invalid source (shortRepo/slug)",
        })
    }
}

/// FR-725: `short_repo` must be a valid name; `slug` non-empty without whitespace or `/`.
fn validate_proposal_source(short_repo: &str, slug: &str) -> Result<(), Rejection> {
    if !is_valid_name(short_repo)
        || slug.is_empty()
        || slug.chars().any(|c| c.is_whitespace() || c == '/')
    {
        return Err(Rejection::InvalidSource);
    }
    Ok(())
}

/// Errors from memory operations.
#[derive(Debug)]
pub enum MemoryError {
    Rejected(Rejection),
    NotFound(String),
    /// Proposal is not pending; carries its current status.
    AlreadyDecided(String),
    /// Queue file exists but is not valid JSON; never overwritten.
    CorruptQueue(String),
    Io(std::io::Error),
}

impl std::fmt::Display for MemoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryError::Rejected(r) => write!(f, "rejected: {r}"),
            MemoryError::NotFound(id) => write!(f, "not found: {id}"),
            MemoryError::AlreadyDecided(s) => write!(f, "already {s}"),
            MemoryError::CorruptQueue(m) => write!(f, "corrupt queue file: {m}"),
            MemoryError::Io(e) => write!(f, "i/o error: {e}"),
        }
    }
}

impl std::error::Error for MemoryError {}

impl From<std::io::Error> for MemoryError {
    fn from(e: std::io::Error) -> Self {
        MemoryError::Io(e)
    }
}

/// One queue item; field names match `memory.mjs` (`queue/proposals.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: String,
    pub short_repo: String,
    pub slug: String,
    pub target: String,
    pub text: String,
    /// `pending` | `approved` | `rejected`.
    pub status: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
}

/// One item in a proposal batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalInput {
    pub target: String,
    pub text: String,
}

/// Result of [`MemoryStore::propose`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProposeOutcome {
    pub added: Vec<Proposal>,
    /// (input, reason)
    pub rejected: Vec<(ProposalInput, String)>,
}

/// Result of a save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved { target: String },
    AlreadySaved { target: String },
}

/// One search hit (FR-730).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    /// Relative target, e.g. `gotchas/windows.md`.
    pub target: String,
    /// The full entry line.
    pub entry: String,
}

/// Validate entry text (FR-723). Returns the trimmed text.
pub fn validate_text(text: &str) -> Result<String, Rejection> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(Rejection::Empty);
    }
    if text.contains('\r') || text.contains('\n') {
        return Err(Rejection::MultiLine);
    }
    if trimmed.chars().count() > MAX_ENTRY_CHARS {
        return Err(Rejection::TooLong);
    }
    if contains_secret(trimmed) {
        return Err(Rejection::Secret);
    }
    Ok(trimmed.to_string())
}

/// True if `text` matches a SECRET_PATTERNS entry (FR-723).
pub fn contains_secret(text: &str) -> bool {
    SECRET_PATTERNS.iter().any(|re| re.is_match(text))
}

/// Short project name from a workspace path (see SPEC terms).
pub fn short_repo(workspace: &Path) -> String {
    let base = workspace.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let lower = base.to_ascii_lowercase();
    let mut normalized = String::new();
    let mut in_dash = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            normalized.push(c);
            in_dash = false;
        } else if !in_dash {
            normalized.push('-');
            in_dash = true;
        }
    }
    let mut trimmed = normalized.trim_matches('-').to_string();
    if trimmed.chars().count() > 24 {
        trimmed = trimmed.chars().take(24).collect();
        trimmed = trimmed.trim_end_matches('-').to_string();
    }
    if trimmed.is_empty() {
        "repo".to_string()
    } else {
        trimmed
    }
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

fn now_utc_parts() -> (i32, u32, u32, u32, u32, u32, u32) {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();
    let days = (total_secs / 86400) as i64;
    let rem_secs = (total_secs % 86400) as u32;
    let hours = rem_secs / 3600;
    let minutes = (rem_secs % 3600) / 60;
    let seconds = rem_secs % 60;
    let (y, m, d) = civil_from_days(days);
    (y, m, d, hours, minutes, seconds, millis)
}

/// Today's UTC date `YYYY-MM-DD` (OQ-720).
pub fn today_utc() -> String {
    let (y, m, d, _, _, _, _) = now_utc_parts();
    format!("{y:04}-{m:02}-{d:02}")
}

/// ISO 8601 UTC timestamp `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn iso_timestamp_utc() -> String {
    let (y, m, d, h, min, s, ms) = now_utc_parts();
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}.{ms:03}Z")
}

fn to_base36(mut n: u128) -> String {
    if n == 0 {
        return "0".to_string();
    }
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut res = Vec::new();
    while n > 0 {
        res.push(CHARS[(n % 36) as usize]);
        n /= 36;
    }
    res.reverse();
    String::from_utf8(res).unwrap()
}

fn random_4_base36() -> String {
    let u = uuid::Uuid::new_v4();
    let bytes = u.as_bytes();
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let c1 = CHARS[(bytes[0] as usize) % 36] as char;
    let c2 = CHARS[(bytes[1] as usize) % 36] as char;
    let c3 = CHARS[(bytes[2] as usize) % 36] as char;
    let c4 = CHARS[(bytes[3] as usize) % 36] as char;
    format!("{c1}{c2}{c3}{c4}")
}

fn generate_proposal_id(queue: &[Proposal]) -> String {
    loop {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let base36_millis = to_base36(millis);
        let rand_part = random_4_base36();
        let id = format!("{base36_millis}-{rand_part}");
        if !queue.iter().any(|q| q.id == id) {
            return id;
        }
    }
}

fn read_entry_lines(path: &Path) -> Vec<String> {
    if !path.exists() {
        return Vec::new();
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if ENTRY_RE.is_match(line) {
                Some(line.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn fit_entries(entries: &[String], budget: usize) -> Vec<String> {
    let mut kept = Vec::new();
    let mut used = 0;
    for entry in entries {
        let len = entry.chars().count();
        if used + len + 1 > budget {
            break;
        }
        kept.push(entry.clone());
        used += len + 1;
    }
    let omitted = entries.len() - kept.len();
    if omitted > 0 {
        kept.push(format!("…({omitted} omitted)"));
    }
    kept
}

fn read_dir_entries(dir: &Path) -> Vec<String> {
    if !dir.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path.extension().and_then(|s| s.to_str()) == Some("md")
            && let Some(file_name) = path.file_name().and_then(|s| s.to_str())
        {
            files.push((file_name.to_string(), path));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut all_entries = Vec::new();
    for (_, path) in files {
        all_entries.extend(read_entry_lines(&path));
    }
    all_entries
}

fn load_queue(path: &Path) -> Result<Vec<Proposal>, MemoryError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str::<Vec<Proposal>>(&content)
        .map_err(|e| MemoryError::CorruptQueue(e.to_string()))
}

fn save_queue(path: &Path, queue: &[Proposal]) -> Result<(), MemoryError> {
    let parent = path.parent().unwrap_or(path);
    std::fs::create_dir_all(parent)?;
    let temp_name = format!(".proposals.json.tmp.{}", uuid::Uuid::new_v4());
    let temp_path = parent.join(temp_name);
    let mut json = serde_json::to_string_pretty(queue)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    json.push('\n');
    std::fs::write(&temp_path, json.as_bytes())?;
    std::fs::rename(&temp_path, path)?;
    Ok(())
}

/// Memory store rooted at `<vault>/<dir>`.
#[derive(Debug, Clone)]
pub struct MemoryStore {
    root: PathBuf,
}

impl MemoryStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/queue/proposals.json`.
    pub fn queue_file(&self) -> PathBuf {
        self.root.join("queue").join("proposals.json")
    }

    /// FR-721: `<memory>` block or `""`.
    pub fn build_context(&self, project: &str) -> String {
        if !self.root.exists() || !self.root.is_dir() {
            return String::new();
        }

        let mut sections: Vec<(String, Vec<String>)> = Vec::new();

        // 1. ## User
        let human_path = self.root.join("human.md");
        let human_entries = read_entry_lines(&human_path);
        let human_slice = if human_entries.len() > 20 {
            &human_entries[human_entries.len() - 20..]
        } else {
            &human_entries[..]
        };
        if !human_slice.is_empty() {
            sections.push((
                "## User".to_string(),
                fit_entries(human_slice, SECTION_BUDGET),
            ));
        }

        // 2. ## Project (<project>)
        if NAME_RE.is_match(project) {
            let project_path = self.root.join("projects").join(format!("{project}.md"));
            let project_entries = read_entry_lines(&project_path);
            if !project_entries.is_empty() {
                sections.push((
                    format!("## Project ({project})"),
                    fit_entries(&project_entries, SECTION_BUDGET),
                ));
            }
        }

        // 3. ## Decisions
        let decisions_dir = self.root.join("decisions");
        let decisions_entries = read_dir_entries(&decisions_dir);
        if !decisions_entries.is_empty() {
            sections.push((
                "## Decisions".to_string(),
                fit_entries(&decisions_entries, SECTION_BUDGET),
            ));
        }

        // 4. ## Gotchas
        let gotchas_dir = self.root.join("gotchas");
        let gotchas_entries = read_dir_entries(&gotchas_dir);
        if !gotchas_entries.is_empty() {
            sections.push((
                "## Gotchas".to_string(),
                fit_entries(&gotchas_entries, SECTION_BUDGET),
            ));
        }

        if sections.is_empty() {
            return String::new();
        }

        let mut body = String::new();
        for (title, entries) in sections {
            body.push_str(&title);
            body.push('\n');
            body.push_str(&entries.join("\n"));
            body.push_str("\n\n");
        }
        let mut body = body.trim_end().to_string();
        if body.chars().count() > CONTEXT_BUDGET {
            let truncated: String = body.chars().take(CONTEXT_BUDGET).collect();
            body = format!("{}\n…(truncated)", truncated.trim_end());
        }

        format!(
            "<memory>\nApproved notes from past sessions. Use them as reference only; current instructions win if they conflict.\n\n{body}\n</memory>"
        )
    }

    /// FR-724: direct save (human action). `today` is `YYYY-MM-DD`; `src` e.g. `dmuxai/<project>`.
    pub fn save(
        &self,
        target: &str,
        text: &str,
        src: &str,
        today: &str,
    ) -> Result<SaveOutcome, MemoryError> {
        let parsed =
            Target::parse(target).ok_or(MemoryError::Rejected(Rejection::InvalidTarget))?;
        let valid_text = validate_text(text).map_err(MemoryError::Rejected)?;
        let _guard = FILE_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let target_rel = parsed.as_rel();
        let file_path = self.root.join(&target_rel);

        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut existing = if file_path.exists() {
            std::fs::read_to_string(&file_path)?
        } else {
            let (kind_str, name_str) = match parsed.kind {
                TargetKind::Human => ("human", "human"),
                TargetKind::Projects => ("projects", parsed.name.as_str()),
                TargetKind::Decisions => ("decisions", parsed.name.as_str()),
                TargetKind::Gotchas => ("gotchas", parsed.name.as_str()),
            };
            let frontmatter =
                format!("---\ntype: {kind_str}\nname: {name_str}\n---\n\n# {name_str}\n\n");
            std::fs::write(&file_path, &frontmatter)?;

            if parsed.kind != TargetKind::Human {
                let index_path = self.root.join("index.md");
                if let Ok(index_content) = std::fs::read_to_string(&index_path) {
                    let link = format!("[[{}/{}]]", kind_str, parsed.name);
                    if !index_content.contains(&link) {
                        let mut new_index = index_content;
                        if !new_index.ends_with('\n') && !new_index.is_empty() {
                            new_index.push('\n');
                        }
                        new_index.push_str(&format!("- {link}\n"));
                        std::fs::write(&index_path, new_index)?;
                    }
                }
            }

            frontmatter
        };

        let mut modified = false;
        for placeholder in ["(아직 기록 없음)", "(no entries yet)"] {
            if existing.contains(placeholder) {
                let lines: Vec<&str> = existing.lines().collect();
                let filtered: Vec<&str> = lines
                    .into_iter()
                    .filter(|line| line.trim() != placeholder)
                    .collect();
                let mut updated = filtered.join("\n");
                if existing.ends_with('\n') && !updated.is_empty() {
                    updated.push('\n');
                }
                existing = updated;
                modified = true;
            }
        }
        if modified {
            std::fs::write(&file_path, &existing)?;
        }

        let already_there = existing.lines().any(|line| {
            let l = line.strip_suffix('\r').unwrap_or(line);
            l.contains(&format!("] {valid_text} ")) || l.ends_with(&format!("] {valid_text}"))
        });

        if already_there {
            return Ok(SaveOutcome::AlreadySaved { target: target_rel });
        }

        let line = format!("- [{today}] {valid_text} <!-- src: {src} -->");
        let mut updated = existing;
        if !updated.ends_with('\n') && !updated.is_empty() {
            updated.push('\n');
        }
        updated.push_str(&line);
        updated.push('\n');
        std::fs::write(&file_path, updated)?;

        Ok(SaveOutcome::Saved { target: target_rel })
    }

    /// FR-725: queue proposals. `short_repo`/`slug` are recorded on each item.
    pub fn propose(
        &self,
        short_repo: &str,
        slug: &str,
        items: &[ProposalInput],
    ) -> Result<ProposeOutcome, MemoryError> {
        validate_proposal_source(short_repo, slug).map_err(MemoryError::Rejected)?;
        let _guard = QUEUE_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let queue_path = self.queue_file();
        let mut queue = load_queue(&queue_path)?;
        let mut added = Vec::new();
        let mut rejected = Vec::new();

        for input in items {
            let parsed = match Target::parse(&input.target) {
                Some(t) => t,
                None => {
                    rejected.push((input.clone(), Rejection::InvalidTarget.to_string()));
                    continue;
                }
            };
            let valid_text = match validate_text(&input.text) {
                Ok(t) => t,
                Err(rej) => {
                    rejected.push((input.clone(), rej.to_string()));
                    continue;
                }
            };
            let is_dup = queue.iter().any(|q| {
                q.target == parsed.as_rel() && q.text == valid_text && q.status != "rejected"
            });
            if is_dup {
                rejected.push((input.clone(), "already queued".to_string()));
                continue;
            }
            let id = generate_proposal_id(&queue);
            let proposal = Proposal {
                id,
                short_repo: short_repo.to_string(),
                slug: slug.to_string(),
                target: parsed.as_rel(),
                text: valid_text,
                status: "pending".to_string(),
                created_at: iso_timestamp_utc(),
                decided_at: None,
            };
            queue.push(proposal.clone());
            added.push(proposal);
        }

        if !added.is_empty() {
            save_queue(&queue_path, &queue)?;
        }

        Ok(ProposeOutcome { added, rejected })
    }

    /// FR-725: pending proposals, oldest first.
    pub fn list_pending(&self) -> Result<Vec<Proposal>, MemoryError> {
        let _guard = QUEUE_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let queue = load_queue(&self.queue_file())?;
        let mut pending = queue
            .into_iter()
            .filter(|q| q.status == "pending")
            .collect::<Vec<_>>();
        pending.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(pending)
    }

    /// FR-725: approve and write the entry.
    ///
    /// Writes the markdown entry before updating the queue. If `save_queue` fails after a
    /// successful [`save`], re-approving the same proposal self-heals: the file already
    /// contains the text so the second attempt returns [`SaveOutcome::AlreadySaved`] while
    /// the queue can still be marked approved.
    pub fn approve(&self, id: &str, today: &str) -> Result<SaveOutcome, MemoryError> {
        let _guard = QUEUE_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let queue_path = self.queue_file();
        let mut queue = load_queue(&queue_path)?;
        let item = queue
            .iter_mut()
            .find(|q| q.id == id)
            .ok_or_else(|| MemoryError::NotFound(id.to_string()))?;
        if item.status != "pending" {
            return Err(MemoryError::AlreadyDecided(item.status.clone()));
        }
        Target::parse(&item.target).ok_or(MemoryError::Rejected(Rejection::InvalidTarget))?;
        validate_text(&item.text).map_err(MemoryError::Rejected)?;

        let src = format!("{}/{}", item.short_repo, item.slug);
        let outcome = self.save(&item.target, &item.text, &src, today)?;
        item.status = "approved".to_string();
        item.decided_at = Some(iso_timestamp_utc());
        save_queue(&queue_path, &queue)?;
        Ok(outcome)
    }

    /// FR-725: reject.
    pub fn reject(&self, id: &str) -> Result<(), MemoryError> {
        let _guard = QUEUE_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let queue_path = self.queue_file();
        let mut queue = load_queue(&queue_path)?;
        let item = queue
            .iter_mut()
            .find(|q| q.id == id)
            .ok_or_else(|| MemoryError::NotFound(id.to_string()))?;
        if item.status != "pending" {
            return Err(MemoryError::AlreadyDecided(item.status.clone()));
        }
        item.status = "rejected".to_string();
        item.decided_at = Some(iso_timestamp_utc());
        save_queue(&queue_path, &queue)?;
        Ok(())
    }

    /// FR-730: case-insensitive all-terms search; `limit` clamped to 1..=100.
    ///
    /// A whitespace-only `query` yields an empty list; callers (MCP, dashboard) must map
    /// that to an error response (`isError` / HTTP 400) per FR-730.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let query_trimmed = query.trim();
        if query_trimmed.is_empty() {
            return Vec::new();
        }
        let terms: Vec<String> = query.split_whitespace().map(|s| s.to_lowercase()).collect();
        let limit = limit.clamp(1, 100);

        let mut files: Vec<(String, PathBuf)> = Vec::new();

        // 1. human.md
        let human_path = self.root.join("human.md");
        if human_path.is_file() {
            files.push(("human.md".to_string(), human_path));
        }

        // 2. projects/*.md, decisions/*.md, gotchas/*.md
        for kind in ["projects", "decisions", "gotchas"] {
            let dir = self.root.join(kind);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut sub_files = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && path.extension().and_then(|s| s.to_str()) == Some("md")
                    && let Some(file_name) = path.file_name().and_then(|s| s.to_str())
                {
                    sub_files.push((file_name.to_string(), path));
                }
            }
            sub_files.sort_by(|a, b| a.0.cmp(&b.0));
            for (file_name, path) in sub_files {
                files.push((format!("{kind}/{file_name}"), path));
            }
        }

        let mut hits = Vec::new();
        for (rel_target, path) in files {
            let lines = read_entry_lines(&path);
            for line in lines.into_iter().rev() {
                let lower = line.to_lowercase();
                if terms.iter().all(|term| lower.contains(term)) {
                    hits.push(SearchHit {
                        target: rel_target.clone(),
                        entry: line,
                    });
                    if hits.len() >= limit {
                        return hits;
                    }
                }
            }
        }

        hits
    }
}

/// Everything a front end (TUI, dashboard, MCP) needs for one workspace.
#[derive(Debug, Clone)]
pub struct MemoryHandle {
    pub store: MemoryStore,
    /// Effective project name (`[memory] project` or [`short_repo`]).
    pub project: String,
    /// Configured relative `dir` (safe to display; never the absolute root).
    pub dir: String,
    /// `[memory] inject`.
    pub inject: bool,
}

impl MemoryHandle {
    pub fn from_config(config: &MemoryConfig, workspace: &Path) -> Self {
        Self {
            store: MemoryStore::new(config.root.clone()),
            project: config
                .project
                .clone()
                .unwrap_or_else(|| short_repo(workspace)),
            dir: config.dir.clone(),
            inject: config.inject,
        }
    }

    /// Default target `projects/<project>.md`.
    pub fn project_target(&self) -> String {
        format!("projects/{}.md", self.project)
    }

    /// Source tag written into saved lines: `dmuxai/<project>`.
    pub fn src(&self) -> String {
        format!("dmuxai/{}", self.project)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // TC-721 (FR-721): empty or missing root returns empty string.
    #[test]
    fn context_empty_or_missing_root_returns_empty_string() {
        let missing = MemoryStore::new(PathBuf::from("/nonexistent/vault/root"));
        assert_eq!(missing.build_context("dmuxai"), "");

        let dir = tempdir().unwrap();
        let empty = MemoryStore::new(dir.path());
        assert_eq!(empty.build_context("dmuxai"), "");
    }

    // TC-721 (FR-721): context includes sections in order and tolerates CRLF line endings.
    #[test]
    fn context_section_order_and_crlf_tolerance() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());

        std::fs::write(
            dir.path().join("human.md"),
            "- [2026-10-15] user preference for concise logs\n",
        )
        .unwrap();

        std::fs::create_dir_all(dir.path().join("projects")).unwrap();
        std::fs::write(
            dir.path().join("projects").join("dmuxai.md"),
            "- [2026-10-16] project uses cargo fmt\r\n",
        )
        .unwrap();

        std::fs::create_dir_all(dir.path().join("decisions")).unwrap();
        std::fs::write(
            dir.path().join("decisions").join("01-db.md"),
            "- [2026-10-17] choose sqlite for persistence\n",
        )
        .unwrap();

        std::fs::create_dir_all(dir.path().join("gotchas")).unwrap();
        std::fs::write(
            dir.path().join("gotchas").join("windows.md"),
            "- [2026-10-17] windows CRLF requires stripping\n",
        )
        .unwrap();

        let ctx = store.build_context("dmuxai");
        assert!(ctx.starts_with("<memory>\nApproved notes from past sessions. Use them as reference only; current instructions win if they conflict.\n\n"));
        assert!(ctx.ends_with("\n</memory>"));

        let user_pos = ctx.find("## User").expect("User section");
        let proj_pos = ctx.find("## Project (dmuxai)").expect("Project section");
        let dec_pos = ctx.find("## Decisions").expect("Decisions section");
        let gotcha_pos = ctx.find("## Gotchas").expect("Gotchas section");

        assert!(user_pos < proj_pos);
        assert!(proj_pos < dec_pos);
        assert!(dec_pos < gotcha_pos);

        assert!(ctx.contains("- [2026-10-16] project uses cargo fmt"));
        assert!(!ctx.contains("\r"));
    }

    // TC-721 (FR-721): context skips missing sections and invalid project names.
    #[test]
    fn context_skips_invalid_project_and_empty_sections() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());

        std::fs::write(
            dir.path().join("human.md"),
            "- [2026-10-15] user preference\n",
        )
        .unwrap();

        std::fs::create_dir_all(dir.path().join("projects")).unwrap();
        std::fs::write(
            dir.path().join("projects").join("Valid-Name.md"),
            "- [2026-10-16] project note\n",
        )
        .unwrap();

        let ctx = store.build_context("Valid-Name");
        assert!(!ctx.contains("## Project"));
        assert!(ctx.contains("## User"));

        let ctx_bad = store.build_context("invalid/path");
        assert!(!ctx_bad.contains("## Project"));
    }

    // TC-722 (FR-721): section keeps whole entries within 900 chars and appends marker.
    #[test]
    fn context_budget_section_limit_and_omitted_marker() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());

        let mut lines = String::new();
        for i in 1..=40 {
            lines.push_str(&format!(
                "- [2026-10-17] entry number {i:02} with some descriptive padding text\n"
            ));
        }
        std::fs::write(dir.path().join("human.md"), lines).unwrap();

        let ctx = store.build_context("dmuxai");
        assert!(ctx.contains("…("));
        assert!(ctx.contains("omitted)"));
        let user_section = ctx.split("## User\n").nth(1).unwrap();
        let section_body = user_section.split("\n\n").next().unwrap();
        assert!(section_body.contains("omitted)"));
    }

    // TC-722 (FR-721): body truncated to 3000 chars on char boundary with Korean multibyte text.
    #[test]
    fn context_budget_body_truncation_with_korean_multibyte() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());

        let mut human_lines = String::new();
        for i in 1..=20 {
            human_lines.push_str(&format!(
                "- [2026-10-17] 한국어 사용자 설정 기록 번호 {i:02} 가나다라마바사아자차카타파하\n"
            ));
        }
        std::fs::write(dir.path().join("human.md"), human_lines).unwrap();

        std::fs::create_dir_all(dir.path().join("projects")).unwrap();
        let mut proj_lines = String::new();
        for i in 1..=20 {
            proj_lines.push_str(&format!(
                "- [2026-10-17] 한국어 프로젝트 기록 번호 {i:02} 가나다라마바사아자차카타파하\n"
            ));
        }
        std::fs::write(dir.path().join("projects").join("dmuxai.md"), proj_lines).unwrap();

        std::fs::create_dir_all(dir.path().join("decisions")).unwrap();
        std::fs::create_dir_all(dir.path().join("gotchas")).unwrap();

        let mut decision_lines = String::new();
        for i in 1..=20 {
            decision_lines.push_str(&format!(
                "- [2026-10-17] 한국어 결정 사항 기록 번호 {i:02} 가나다라마바사아자차카타파하\n"
            ));
        }
        std::fs::write(
            dir.path().join("decisions").join("decisions-korean.md"),
            decision_lines,
        )
        .unwrap();

        let mut gotcha_lines = String::new();
        for i in 1..=20 {
            gotcha_lines.push_str(&format!(
                "- [2026-10-17] 한국어 함정 및 주의사항 번호 {i:02} 다람쥐 헌 쳇바퀴에 타고파\n"
            ));
        }
        std::fs::write(
            dir.path().join("gotchas").join("gotchas-korean.md"),
            gotcha_lines,
        )
        .unwrap();

        let ctx = store.build_context("dmuxai");
        assert!(ctx.contains("…(truncated)"));
        assert!(ctx.ends_with("\n</memory>"));
        assert!(ctx.contains("한국어"));
    }

    // TC-723 (FR-723): empty and multi-line strings are rejected.
    #[test]
    fn validate_text_empty_and_multiline() {
        assert_eq!(validate_text(""), Err(Rejection::Empty));
        assert_eq!(validate_text("   "), Err(Rejection::Empty));
        assert_eq!(validate_text("\n"), Err(Rejection::Empty));
        assert_eq!(validate_text("line 1\nline 2"), Err(Rejection::MultiLine));
        assert_eq!(validate_text("line 1\r\nline 2"), Err(Rejection::MultiLine));
        assert_eq!(validate_text("line\r"), Err(Rejection::MultiLine));
    }

    // TC-723 (FR-723): length checked by Unicode chars: 201 rejected, 200 Korean accepted.
    #[test]
    fn validate_text_length_and_korean_char_count() {
        let s200 = "a".repeat(200);
        assert_eq!(validate_text(&s200), Ok(s200));

        let s201 = "a".repeat(201);
        assert_eq!(validate_text(&s201), Err(Rejection::TooLong));

        let s200_korean = "가".repeat(200);
        assert_eq!(validate_text(&s200_korean), Ok(s200_korean));

        let s201_korean = "가".repeat(201);
        assert_eq!(validate_text(&s201_korean), Err(Rejection::TooLong));
    }

    // TC-723 (FR-723): each secret pattern is rejected.
    #[test]
    fn validate_text_secret_patterns() {
        assert_eq!(
            validate_text("my password is secret"),
            Err(Rejection::Secret)
        );
        assert_eq!(validate_text("check passwd entry"), Err(Rejection::Secret));
        assert_eq!(validate_text("비밀번호 변경 알림"), Err(Rejection::Secret));
        assert_eq!(validate_text("a secret note"), Err(Rejection::Secret));
        assert_eq!(validate_text("pass the token here"), Err(Rejection::Secret));
        assert_eq!(
            validate_text("the api key is missing"),
            Err(Rejection::Secret)
        );
        assert_eq!(validate_text("api_key = abc"), Err(Rejection::Secret));
        assert_eq!(validate_text("api-key: xyz"), Err(Rejection::Secret));
        assert_eq!(validate_text("apikey 123"), Err(Rejection::Secret));
        assert_eq!(validate_text("bearer eyJhbGciOi"), Err(Rejection::Secret));
        assert_eq!(validate_text("sk-1234567890abcdef"), Err(Rejection::Secret));
        assert_eq!(
            validate_text("ghp_1234567890abcdef"),
            Err(Rejection::Secret)
        );
        assert_eq!(
            validate_text("-----BEGIN RSA PRIVATE KEY-----"),
            Err(Rejection::Secret)
        );
        assert_eq!(
            validate_text("-----BEGIN PRIVATE KEY-----"),
            Err(Rejection::Secret)
        );

        assert_eq!(
            validate_text("tokenize input"),
            Ok("tokenize input".to_string())
        );
        assert_eq!(
            validate_text("betoken something"),
            Ok("betoken something".to_string())
        );
    }

    // TC-723 (FR-723): valid and invalid target paths.
    #[test]
    fn validate_target_syntax() {
        assert_eq!(
            Target::parse("human.md"),
            Some(Target {
                kind: TargetKind::Human,
                name: "human".to_string(),
            })
        );
        assert_eq!(
            Target::parse("projects/my-repo.md"),
            Some(Target {
                kind: TargetKind::Projects,
                name: "my-repo".to_string(),
            })
        );
        assert_eq!(
            Target::parse("decisions/01_sqlite.md"),
            Some(Target {
                kind: TargetKind::Decisions,
                name: "01_sqlite".to_string(),
            })
        );
        assert_eq!(
            Target::parse("gotchas/crlf-handling.md"),
            Some(Target {
                kind: TargetKind::Gotchas,
                name: "crlf-handling".to_string(),
            })
        );

        assert_eq!(Target::parse("../x.md"), None);
        assert_eq!(Target::parse("projects/A.md"), None);
        assert_eq!(Target::parse("projects/a/b.md"), None);
        assert_eq!(Target::parse("projects/a\\b.md"), None);
        assert_eq!(Target::parse("other/file.md"), None);
        assert_eq!(Target::parse("projects/.md"), None);
        assert_eq!(Target::parse("projects/-bad.md"), None);
    }

    // TC-724 (FR-724): save creates missing file with frontmatter and adds index link.
    #[test]
    fn save_creates_file_frontmatter_and_index_link() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());

        std::fs::write(dir.path().join("index.md"), "# Index\n\n").unwrap();

        let outcome = store
            .save(
                "projects/dmuxai.md",
                "first entry",
                "dmuxai/main",
                "2026-10-17",
            )
            .unwrap();
        assert_eq!(
            outcome,
            SaveOutcome::Saved {
                target: "projects/dmuxai.md".to_string(),
            }
        );

        let content =
            std::fs::read_to_string(dir.path().join("projects").join("dmuxai.md")).unwrap();
        assert!(content.starts_with("---\ntype: projects\nname: dmuxai\n---\n\n# dmuxai\n\n"));
        assert!(content.contains("- [2026-10-17] first entry <!-- src: dmuxai/main -->\n"));

        let index = std::fs::read_to_string(dir.path().join("index.md")).unwrap();
        assert!(index.contains("- [[projects/dmuxai]]\n"));
    }

    // TC-724 (FR-724): save removes placeholder and dedups identical entry.
    #[test]
    fn save_dedup_and_placeholder_removal() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());

        std::fs::create_dir_all(dir.path().join("gotchas")).unwrap();
        std::fs::write(
            dir.path().join("gotchas").join("windows.md"),
            "---\ntype: gotchas\nname: windows\n---\n\n# windows\n\n(no entries yet)\n",
        )
        .unwrap();

        let outcome = store
            .save(
                "gotchas/windows.md",
                "windows path quirk",
                "dmuxai/test",
                "2026-10-17",
            )
            .unwrap();
        assert_eq!(
            outcome,
            SaveOutcome::Saved {
                target: "gotchas/windows.md".to_string(),
            }
        );

        let content =
            std::fs::read_to_string(dir.path().join("gotchas").join("windows.md")).unwrap();
        assert!(!content.contains("(no entries yet)"));
        assert!(content.contains("- [2026-10-17] windows path quirk <!-- src: dmuxai/test -->\n"));

        let dedup = store
            .save(
                "gotchas/windows.md",
                "windows path quirk",
                "dmuxai/test2",
                "2026-10-18",
            )
            .unwrap();
        assert_eq!(
            dedup,
            SaveOutcome::AlreadySaved {
                target: "gotchas/windows.md".to_string(),
            }
        );

        let content_after =
            std::fs::read_to_string(dir.path().join("gotchas").join("windows.md")).unwrap();
        assert_eq!(content, content_after);
    }

    // TC-724 (FR-724): human.md gets type human, name human, not in index.md.
    #[test]
    fn save_human_creates_file_without_index_link() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());

        std::fs::write(dir.path().join("index.md"), "# Index\n\n").unwrap();

        let outcome = store
            .save("human.md", "keep tests fast", "human", "2026-10-17")
            .unwrap();
        assert_eq!(
            outcome,
            SaveOutcome::Saved {
                target: "human.md".to_string(),
            }
        );

        let content = std::fs::read_to_string(dir.path().join("human.md")).unwrap();
        assert!(content.starts_with("---\ntype: human\nname: human\n---\n\n# human\n\n"));
        assert!(content.contains("- [2026-10-17] keep tests fast <!-- src: human -->\n"));

        let index = std::fs::read_to_string(dir.path().join("index.md")).unwrap();
        assert!(!index.contains("human"));
    }

    // TC-725 (FR-725): proposals queued with unique id and camelCase JSON fields.
    #[test]
    fn queue_propose_and_json_format() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());

        let input = ProposalInput {
            target: "projects/dmuxai.md".to_string(),
            text: "prefer rg over grep".to_string(),
        };

        let outcome = store
            .propose("dmuxai", "slice-e", std::slice::from_ref(&input))
            .unwrap();
        assert_eq!(outcome.added.len(), 1);
        assert_eq!(outcome.rejected.len(), 0);

        let prop = &outcome.added[0];
        assert_eq!(prop.short_repo, "dmuxai");
        assert_eq!(prop.slug, "slice-e");
        assert_eq!(prop.target, "projects/dmuxai.md");
        assert_eq!(prop.text, "prefer rg over grep");
        assert_eq!(prop.status, "pending");
        assert!(prop.id.contains('-'));

        let queue_raw = std::fs::read_to_string(store.queue_file()).unwrap();
        assert!(queue_raw.contains("\"id\":"));
        assert!(queue_raw.contains("\"shortRepo\": \"dmuxai\""));
        assert!(queue_raw.contains("\"slug\": \"slice-e\""));
        assert!(queue_raw.contains("\"target\": \"projects/dmuxai.md\""));
        assert!(queue_raw.contains("\"text\": \"prefer rg over grep\""));
        assert!(queue_raw.contains("\"status\": \"pending\""));
        assert!(queue_raw.contains("\"createdAt\":"));
        assert!(!queue_raw.contains("\"decidedAt\":"));
    }

    // TC-725 (FR-725): pending list is oldest-first by `createdAt`, then `id`.
    #[test]
    fn queue_list_pending_oldest_first() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());
        std::fs::create_dir_all(dir.path().join("queue")).unwrap();
        let queue_json = r#"[
  {
    "id": "newer-id",
    "shortRepo": "dmuxai",
    "slug": "b",
    "target": "projects/dmuxai.md",
    "text": "second",
    "status": "pending",
    "createdAt": "2026-10-17T12:00:00.000Z"
  },
  {
    "id": "older-id",
    "shortRepo": "dmuxai",
    "slug": "a",
    "target": "projects/dmuxai.md",
    "text": "first",
    "status": "pending",
    "createdAt": "2026-10-17T10:00:00.000Z"
  }
]"#;
        std::fs::write(store.queue_file(), queue_json).unwrap();

        let pending = store.list_pending().unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].id, "older-id");
        assert_eq!(pending[1].id, "newer-id");
    }

    // TC-725 (FR-725): invalid shortRepo/slug rejects the whole batch.
    #[test]
    fn queue_propose_rejects_invalid_source() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());
        let input = ProposalInput {
            target: "projects/dmuxai.md".to_string(),
            text: "valid line".to_string(),
        };

        assert!(matches!(
            store.propose("INVALID", "x", std::slice::from_ref(&input)),
            Err(MemoryError::Rejected(Rejection::InvalidSource))
        ));
        assert!(matches!(
            store.propose("dmuxai", "", std::slice::from_ref(&input)),
            Err(MemoryError::Rejected(Rejection::InvalidSource))
        ));
        assert!(matches!(
            store.propose("dmuxai", "has space", std::slice::from_ref(&input)),
            Err(MemoryError::Rejected(Rejection::InvalidSource))
        ));
        assert!(matches!(
            store.propose("dmuxai", "a/b", std::slice::from_ref(&input)),
            Err(MemoryError::Rejected(Rejection::InvalidSource))
        ));
        assert!(!store.queue_file().exists());
    }

    // TC-725 (FR-725): duplicate proposal with same target and text is rejected.
    #[test]
    fn queue_duplicate_rejected() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());

        let input = ProposalInput {
            target: "projects/dmuxai.md".to_string(),
            text: "always check clippy".to_string(),
        };

        let res1 = store
            .propose("dmuxai", "task-1", std::slice::from_ref(&input))
            .unwrap();
        assert_eq!(res1.added.len(), 1);

        let res2 = store.propose("dmuxai", "task-2", &[input]).unwrap();
        assert_eq!(res2.added.len(), 0);
        assert_eq!(res2.rejected.len(), 1);
        assert_eq!(res2.rejected[0].1, "already queued");
    }

    // TC-725 (FR-725): approve writes entry and marks status; reject marks status.
    #[test]
    fn queue_approve_and_reject() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());

        let res = store
            .propose(
                "dmuxai",
                "feat-1",
                &[
                    ProposalInput {
                        target: "projects/dmuxai.md".to_string(),
                        text: "approve me".to_string(),
                    },
                    ProposalInput {
                        target: "projects/dmuxai.md".to_string(),
                        text: "reject me".to_string(),
                    },
                ],
            )
            .unwrap();

        let id1 = &res.added[0].id;
        let id2 = &res.added[1].id;

        let app_res = store.approve(id1, "2026-10-17").unwrap();
        assert_eq!(
            app_res,
            SaveOutcome::Saved {
                target: "projects/dmuxai.md".to_string(),
            }
        );

        let saved = std::fs::read_to_string(dir.path().join("projects").join("dmuxai.md")).unwrap();
        assert!(saved.contains("- [2026-10-17] approve me <!-- src: dmuxai/feat-1 -->\n"));

        store.reject(id2).unwrap();

        let pending = store.list_pending().unwrap();
        assert!(pending.is_empty());

        match store.approve(id1, "2026-10-17") {
            Err(MemoryError::AlreadyDecided(s)) => assert_eq!(s, "approved"),
            other => panic!("expected AlreadyDecided, got {other:?}"),
        }

        match store.reject(id2) {
            Err(MemoryError::AlreadyDecided(s)) => assert_eq!(s, "rejected"),
            other => panic!("expected AlreadyDecided, got {other:?}"),
        }

        match store.approve("nonexistent", "2026-10-17") {
            Err(MemoryError::NotFound(id)) => assert_eq!(id, "nonexistent"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    // TC-725 (FR-725): approve when the entry already exists marks approved with AlreadySaved.
    #[test]
    fn queue_approve_already_saved_marks_approved() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());

        store
            .save(
                "projects/dmuxai.md",
                "already on disk",
                "dmuxai/manual",
                "2026-10-17",
            )
            .unwrap();

        let proposed = store
            .propose(
                "dmuxai",
                "queue",
                &[ProposalInput {
                    target: "projects/dmuxai.md".to_string(),
                    text: "already on disk".to_string(),
                }],
            )
            .unwrap();
        let id = proposed.added[0].id.clone();

        let outcome = store.approve(&id, "2026-10-18").unwrap();
        assert_eq!(
            outcome,
            SaveOutcome::AlreadySaved {
                target: "projects/dmuxai.md".to_string(),
            }
        );

        let queue: Vec<Proposal> =
            serde_json::from_str(&std::fs::read_to_string(store.queue_file()).unwrap()).unwrap();
        let item = queue.iter().find(|q| q.id == id).unwrap();
        assert_eq!(item.status, "approved");
        assert!(item.decided_at.is_some());
    }

    // TC-725 (FR-725): corrupt proposals.json returns CorruptQueue and is left byte-identical.
    #[test]
    fn queue_corrupt_file_is_never_overwritten() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());

        std::fs::create_dir_all(dir.path().join("queue")).unwrap();
        let corrupt_bytes = b"INVALID JSON CONTENT {{{";
        std::fs::write(store.queue_file(), corrupt_bytes).unwrap();

        assert!(matches!(
            store.list_pending(),
            Err(MemoryError::CorruptQueue(_))
        ));

        let input = ProposalInput {
            target: "projects/dmuxai.md".to_string(),
            text: "test".to_string(),
        };
        assert!(matches!(
            store.propose("dmuxai", "t", &[input]),
            Err(MemoryError::CorruptQueue(_))
        ));

        assert!(matches!(
            store.approve("some-id", "2026-10-17"),
            Err(MemoryError::CorruptQueue(_))
        ));

        assert!(matches!(
            store.reject("some-id"),
            Err(MemoryError::CorruptQueue(_))
        ));

        let disk_bytes = std::fs::read(store.queue_file()).unwrap();
        assert_eq!(disk_bytes, corrupt_bytes);
    }

    // TC-730 (FR-730): search splits on whitespace, requires all terms, respects order and limit.
    #[test]
    fn search_terms_ordering_and_clamping() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());

        std::fs::write(
            dir.path().join("human.md"),
            "- [2026-10-15] old human entry\n- [2026-10-17] new human entry\n",
        )
        .unwrap();

        std::fs::create_dir_all(dir.path().join("gotchas")).unwrap();
        std::fs::write(
            dir.path().join("gotchas").join("windows.md"),
            "- [2026-10-17] windows path quirk\n",
        )
        .unwrap();

        std::fs::create_dir_all(dir.path().join("decisions")).unwrap();
        std::fs::write(
            dir.path().join("decisions").join("01-db.md"),
            "- [2026-10-16] use sqlite database\n",
        )
        .unwrap();

        let human_hits = store.search("human", 10);
        assert_eq!(human_hits.len(), 2);
        assert_eq!(human_hits[0].entry, "- [2026-10-17] new human entry");
        assert_eq!(human_hits[1].entry, "- [2026-10-15] old human entry");

        let multi_hits = store.search("WINDOWS quirk", 10);
        assert_eq!(multi_hits.len(), 1);
        assert_eq!(multi_hits[0].target, "gotchas/windows.md");
        assert_eq!(multi_hits[0].entry, "- [2026-10-17] windows path quirk");

        let none_hits = store.search("missing term", 10);
        assert!(none_hits.is_empty());

        let empty_hits = store.search("   ", 10);
        assert!(empty_hits.is_empty());

        let clamped_hits = store.search("entry", 1);
        assert_eq!(clamped_hits.len(), 1);
    }

    // TC-720 (FR-720): short repo derived correctly.
    #[test]
    fn short_repo_derivation() {
        assert_eq!(short_repo(Path::new("/a/b/c/dmuxai")), "dmuxai");
        assert_eq!(short_repo(Path::new("")), "repo");
        assert_eq!(short_repo(Path::new("---")), "repo");
        assert_eq!(
            short_repo(Path::new("/workspace/My_Project--123")),
            "my-project-123"
        );
        let long = short_repo(Path::new(
            "/workspace/really-long-project-name-that-exceeds-twenty-four-chars",
        ));
        assert!(long.len() <= 24);
        assert!(!long.ends_with('-'));
    }

    // NFR-731: resolve_root rejects bad dirs and normalizes separators.
    #[test]
    fn resolve_root_rejects_absolute_and_dotdot() {
        let vault = tempdir().unwrap();

        let (root, dir) = resolve_root(vault.path(), None).unwrap();
        assert_eq!(dir, DEFAULT_DIR);
        assert_eq!(root, vault.path().join("cursor-workflow").join("memory"));

        let (root2, dir2) = resolve_root(vault.path(), Some("notes\\memory")).unwrap();
        assert_eq!(dir2, "notes/memory");
        assert_eq!(root2, vault.path().join("notes").join("memory"));

        let (root3, dir3) = resolve_root(vault.path(), Some("")).unwrap();
        assert_eq!(dir3, DEFAULT_DIR);
        assert_eq!(root3, root);

        assert!(resolve_root(vault.path(), Some("/absolute")).is_err());
        assert!(resolve_root(vault.path(), Some("..")).is_err());
        assert!(resolve_root(vault.path(), Some("a/../b")).is_err());

        let missing = PathBuf::from("/nonexistent/dmuxai-memory-vault");
        assert!(resolve_root(&missing, None).is_err());
    }

    // OQ-720: today_utc returns YYYY-MM-DD.
    #[test]
    fn today_utc_format() {
        let today = today_utc();
        let re = Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap();
        assert!(re.is_match(&today));

        let iso = iso_timestamp_utc();
        let iso_re = Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$").unwrap();
        assert!(iso_re.is_match(&iso));
    }
}
