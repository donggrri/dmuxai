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
