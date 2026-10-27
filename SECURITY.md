# Security policy

## Supported versions

Security fixes are provided for the **latest release line** on the default branch (currently
the most recent `v0.2.x` tag and subsequent patch releases). Older release lines do not
receive routine security maintenance unless noted in a release announcement.

## Reporting a vulnerability

**Do not** open a public GitHub issue for security-sensitive reports.

Use GitHub **private vulnerability reporting**:

1. Open the repository on GitHub.
2. Go to the **Security** tab.
3. Choose **Report a vulnerability** and submit details through the private form.

Include enough information to reproduce the issue (affected commands, configuration, platform,
and impact). We will acknowledge reports and coordinate disclosure after a fix is available.

## Secrets in the repository

Continuous integration runs [gitleaks](https://github.com/gitleaks/gitleaks) on the full git
history. Known test fixtures are allowlisted in [.gitleaks.toml](.gitleaks.toml); do not add
real credentials, API keys, or private keys to the tree or to commit messages.

## Workspace configuration trust

A repository's workspace configuration (`dmuxai.toml`) cannot run commands, reference secret environment variables, or set outbound URLs until the user trusts it with `dmuxai trust`. Restricted keys include:
- `notify.command`, `notify.webhook_url`, `notify.webhook_secret_env`, `notify.secret_header`
- `agents.custom`
- `mcp.servers`
- `skills.sources.<name>` (remote git sources or paths outside the workspace root)
- `memory.vault` and `memory.dir` (when resolving outside the workspace root or using path traversal)
- `todo.jira_base_url`
- `runners.<id>.system_prompt_file` and `runners.<id>.models.<model>.system_prompt_file` (when resolving outside the workspace root)

Trust is pinned to the SHA-256 hash of the configuration file and stored in the user's state directory (`trusted-workspaces.json`). The trust hash covers only `dmuxai.toml` itself, not files it references (skills, prompt files); those are confined to the workspace while untrusted and are not hashed. Trust store operations require an existing state or home directory and enforce 0700 state directory and 0600 file permissions, ignoring store files with group/world-writable permissions. Editing the file invalidates trust until approved again. Saving workspace settings from a write-capable dashboard (`dmuxai --web`) refreshes an existing trust record to the new file hash only when the file on disk still matches that record right before the save; it never creates trust, and view-only dashboards and the one-process opt-in never refresh it. Run `dmuxai trust --revoke` to revoke trust regardless of config content. Interactive approval prompts display actual restricted key values (showing secret environment variable names only, never values). Child processes do not inherit the `DMUXAI_TRUST_WORKSPACE_CONFIG` opt-in. Headless invocations never prompt interactively; they can opt in for a single process without modifying the trust store using the `--trust-workspace-config` CLI flag or `DMUXAI_TRUST_WORKSPACE_CONFIG=1` environment variable.

