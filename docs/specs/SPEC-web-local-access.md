# SPEC-web-local-access

Status: In progress · Series: global web view 1/3 (then SPEC-web-default-mode, SPEC-web-all-projects) · Request: t754u, t755u, t758u, t760u

## 1. Decision summary
The dashboard token gets in the way: the owner has to copy a fresh URL every start, and local agents (Grok Bot) often cannot read the dashboard because they lack the token. This change makes a dashboard bound to a loopback address work without a token by default, keeps the token mandatory for any non-loopback bind, and hardens the request checks so other web sites still cannot call the API (CSRF, DNS rebinding). Under WSL2 the Windows browser on the same PC also works without a token, both through WSL localhost forwarding and, in NAT mode, through the WSL IP. A user-level discovery file tells humans and agents where the single dashboard is, from any project path. `[web] require_token = true` restores today's behavior.

## 2. Scope

In scope: token policy, Host/Origin/Fetch-Metadata checks, same-OS-user peer check on Linux, WSL host trust, printed URL, dashboard UI without a token, user-level discovery file, `status --json`, README/SECURITY/API docs.
Out of scope: default UI mode and reusing a running dashboard (SPEC-web-default-mode), the all-projects view (SPEC-web-all-projects), TLS, multi-user auth.

### Assumptions and open questions
| ID | Question | Default chosen in this spec |
|---|---|---|
| OQ-2950 | Should tokenless be the default, or opt-in? | Default tokenless on loopback (owner request t755u). `[web] require_token = true` or `DMUXAI_WEB_REQUIRE_TOKEN=1` turns it back on. |
| OQ-2951 | Same-OS-user check for loopback peers | Linux: when the peer socket is listed in `/proc/net/tcp{,6}`, its uid must be the server's uid or 0 (root; WSL relays run as root). Peers not listed there (WSL2 localhost forwarding or mirrored mode, other network namespaces) are allowed. This is best effort; the residual risk is documented. Confirm. |
| OQ-2952 | Location of the user-level discovery file | `<state base>/web.json` (`state_path::state_base()`: `$DMUXAI_STATE_DIR`, else `$XDG_STATE_HOME/dmuxai`, else `~/.local/state/dmuxai`), file 0600, directory 0700. |
| OQ-2953 | WSL2 NAT mode: which peer is "the Windows host"? | Only in WSL2 NAT mode (Terms): the gateway of the default route in `/proc/net/route` (the Windows `vEthernet (WSL)` address). `/etc/resolv.conf` is not used. Mirrored, bridged and WSL1 get no extra listener (there the default-route IP is the LAN). `[web] trust_wsl_host = false` turns this off. Residual risk: anything Windows forwards to WSL with the gateway as source (e.g. a `netsh portproxy` the user set up, Windows-hosted containers) and L2 spoofing by sibling Hyper-V VMs is treated as local; documented. |
| OQ-2954 | GET requests from non-browser clients (curl, agents) | Allowed without a token and without extra headers: CORS blocks cross-origin reads, the Host check blocks DNS rebinding, and GET `/api/*` handlers are side-effect free (FR-2954). Browsers send Fetch Metadata only to potentially trustworthy origins (`localhost`, `127.0.0.0/8`, `[::1]`), so on `http://<wsl-ip>` the `Sec-Fetch-Site` check is only defense in depth and the write guard relies on `Origin` (always sent on cross-origin POST) and the custom header. |

## 3. Terms
- Loopback bind: an address in `127.0.0.0/8` or `::1`.
- Explicit token: `DMUXAI_WEB_TOKEN` set by the user. The internal re-exec after a workspace switch passes the token and the token-required state separately (`DMUXAI_WEB_TOKEN_REQUIRED=0|1`), so a re-exec never turns a tokenless dashboard into a token one.
- Token required: `require_token = true`, `DMUXAI_WEB_REQUIRE_TOKEN=1`, an explicit token, or a non-loopback bind.
- Guarded paths: `/api/*`. The capability-ticket routes `/docs/{view,download,open}/{ticket}` keep their current behavior (no token, single-use or short-lived tickets) and are out of scope.
- Allowed hosts (loopback bind on port P): `127.0.0.1:P`, `localhost:P`, `[::1]:P`, `<bound ip>:P`; when P is 80 also the same names without `:80`.
- WSL: `/proc/version` contains `microsoft` (case-insensitive) or `WSL_DISTRO_NAME` is set.
- WSL2 NAT mode: WSL, and `wslinfo --networking-mode` prints `nat`; when `wslinfo` is missing, `/proc/version` contains `WSL2` and the default-route interface address is in `172.16.0.0/12` or `192.168.0.0/16` with the gateway in the same /20 or /24.
- WSL host peer: the peer IP equals the Windows host address found per OQ-2953.

## 4. Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-2950 | Must | With a loopback bind and the token not required, guarded paths are served without a token. A request that does carry the right token is still accepted; a wrong token is rejected with 401 (no silent downgrade). |
| FR-2951 | Must | When the token is required, behavior is unchanged from today: `/api/*` without a valid token returns 401. A non-loopback bind (`--bind` + `--allow-remote`) always requires the token, whatever the config says. A workspace-switch re-exec keeps the token-required state of the parent. |
| FR-2952 | Must | On a loopback bind every request, with or without token, must have a `Host` in the allowed hosts (DNS-rebinding guard); otherwise 403. |
| FR-2953 | Must | A tokenless request with a state-changing method (POST, PUT, PATCH, DELETE) on a guarded path is accepted only when either (a) `Origin` equals `http://<Host>` and `Sec-Fetch-Site`, if present, is `same-origin` or `none`, or (b) neither `Origin` nor `Sec-Fetch-Site` is present and `X-Dmuxai-Client: 1` is sent. `Origin: null` is rejected. Otherwise 403. |
| FR-2954 | Must | A tokenless GET on a guarded path with `Sec-Fetch-Site` `cross-site` or `same-site` is rejected with 403. Top-level navigation to `/` (the page shell) and the ticket routes are not blocked. Responses never carry permissive CORS headers. GET `/api/*` handlers are side-effect free (no prompt, no file open, no state write); a test lists the GET `/api/*` routes and any with side effects is moved under FR-2953. |
| FR-2955 | Should | On Linux, for tokenless requests on a loopback listener, if the peer's TCP socket appears in `/proc/net/tcp` or `/proc/net/tcp6` with a uid other than the server's uid or 0, the request is rejected with 403. A peer not found is allowed (OQ-2951). With a valid token the check is skipped. The lookup runs once per accepted connection. |
| FR-2956 | Must | The printed dashboard URL (`dmuxai dashboard: ...`, `serve`, `--web`, local notice) has no `?token=` part when the token is not required, and keeps it when it is. Secrets are never printed in logs or events. |
| FR-2957 | Must | A running dashboard writes a user-level discovery file `<state base>/web.json` (0600, atomic rename) with `base_url`, `windows_url` (FR-2960, when set), `port`, `pid`, `uid`, `mode` (`web` or `serve`), `token_required`, `token` (only when required), `workspaces` (id and canonical path, updated on workspace open and close), and `started_at_ms`. A `serve` or second dashboard never overwrites the file while the pid in it is alive and is a `web` host. The file is removed on clean exit by its owner pid only; readers ignore a file whose pid is dead. The per-workspace `.dmuxai/web.json` stays as today. Tests isolate the file with `DMUXAI_STATE_DIR`. |
| FR-2958 | Must | `dmuxai status --json` (and plain `status`, which prints the same JSON) reports `dashboard` from the per-workspace file when its pid is alive, else from the user-level file, with one schema: `url` (never with the secret), `port`, `pid`, `process_alive`, `token_required`, `source` (`workspace` or `user`), plus the existing `token_file` when present. The per-workspace `.dmuxai/web.json` gains `token_required`. |
| FR-2959 | Must | WSL2 localhost forwarding: a Windows browser reaching the WSL dashboard with Host `localhost:<port>` or `127.0.0.1:<port>` and Origin `http://localhost:<port>` or `http://127.0.0.1:<port>`, whose peer is not visible in `/proc/net/tcp` or is uid 0, is accepted without a token and without `--allow-remote`, for GET and for UI POSTs. |
| FR-2960 | Must | In WSL2 NAT mode, with a loopback bind, no `--bind`, and `[web] trust_wsl_host` unset or `true`, the dashboard also listens on the IPv4 address of the default-route interface on the same port and prints `dmuxai dashboard (Windows): http://<wsl-ip>:<port>/` as a second line. If that bind fails, a warning is printed and loopback keeps working. Not done outside WSL2 NAT mode or when `trust_wsl_host = false`. |
| FR-2961 | Must | On the WSL address listener, a request whose peer IP is the WSL host peer is treated like a loopback request (tokenless unless the token is required). Any other peer on that listener needs a valid token (401 otherwise), whatever `require_token` says. FR-2955 does not apply there. |
| FR-2962 | Must | On the WSL address listener the allowed hosts are `<wsl-ip>:<port>` plus the loopback names, and FR-2953/FR-2954 apply with that Host (writes need a matching `Origin` or the custom header). |
| FR-2963 | Must | WSL and NAT-mode detection (`/proc/version`, `WSL_DISTRO_NAME`, `wslinfo` output), the Windows host address (`/proc/net/route`), interface addresses, and the peer address are read through an injectable source so tests can mock them; malformed or missing files mean "no WSL host peer" (token-only access on that listener). |
| FR-2964 | Must | The dashboard UI works without a token: no JS module stops with "Missing access token" when the token is not required; it sends `X-Dmuxai-Client: 1` and the token only when it has one; the docs viewer does not require a stored token. |
| NFR-2950 | Must | README (`Dashboard token` section), SECURITY.md and docs/API.md state: the default is tokenless on loopback; the tradeoff is that any local process of this machine (and, outside the FR-2955 check, possibly other local users) can call the API; in WSL2 NAT mode any process on the Windows side (and anything Windows forwards from the gateway address, see OQ-2953) can call it, while other peers need the token; how to turn it off (`require_token`, `trust_wsl_host`); and the WSL2 setup (open `http://localhost:<port>/` or the printed Windows URL in the Windows browser, no token, no `--allow-remote`). |
| NFR-2951 | Must | Normal turns and requests have no measurable overhead: the peer lookup is per connection, not per request; the series benchmark (bench harness used for #301) stays within 10% of main and the numbers are reported in the PR. |

## 5. Acceptance scenarios and test design
| TC | Covers | Test |
|---|---|---|
| TC-2950 | FR-2950 | `tc_2950_loopback_api_without_token_is_accepted_and_wrong_token_rejected` |
| TC-2951 | FR-2951 | `tc_2951_require_token_config_env_token_remote_bind_and_reexec_enforce_token` |
| TC-2952 | FR-2952 | `tc_2952_host_allowlist_covers_bound_ip_and_port_80_and_rejects_foreign` |
| TC-2953 | FR-2953 | `tc_2953_tokenless_post_needs_same_origin_or_client_header_and_rejects_null_origin` |
| TC-2954 | FR-2954 | `tc_2954_tokenless_cross_site_get_rejected_shell_and_tickets_allowed_get_routes_side_effect_free` |
| TC-2955 | FR-2955, NFR-2951 | `tc_2955_proc_net_tcp_peer_uid_parsing_mismatch_rejected_root_allowed_once_per_connection` |
| TC-2956 | FR-2956 | `tc_2956_printed_url_has_no_token_when_not_required` |
| TC-2957 | FR-2957 | `tc_2957_user_level_discovery_written_0600_not_clobbered_removed_and_stale_ignored` |
| TC-2958 | FR-2958 | `tc_2958_status_reports_one_dashboard_schema_from_workspace_or_user_file` |
| TC-2959 | FR-2959 | `tc_2959_wsl_localhost_forwarding_hosts_and_origins_accepted_tokenless` |
| TC-2960 | FR-2960 | `tc_2960_wsl2_nat_detection_and_extra_listener_only_in_nat_mode` |
| TC-2961 | FR-2961 | `tc_2961_wsl_listener_trusts_windows_host_peer_and_requires_token_for_others` |
| TC-2962 | FR-2962 | `tc_2962_wsl_listener_host_and_origin_allowlist` |
| TC-2963 | FR-2963 | `tc_2963_wsl_host_from_mocked_route_and_wslinfo_malformed_is_none` |
| TC-2964 | FR-2964 | `tc_2964_dashboard_assets_work_without_token_and_send_client_header` |
| TC-2965 | NFR-2950 | `tc_2965_docs_state_tokenless_default_tradeoff_wsl_and_opt_out` |

## 6. Data and safety contracts
Security: tokenless only on loopback and for the WSL host peer; Host check always on those listeners; Fetch-Metadata/Origin/custom-header CSRF guard for writes; same-uid peer check where visible. Tradeoff documented (NFR-2950).
Persistence: new user-level `web.json` only.
Existing tests that assume a token on every request are updated to set `require_token = true` or to use the tokenless path explicitly: `tc_521`, `tc_522`, `tc_879`, `tc_531` (also gets `DMUXAI_STATE_DIR`), `tests/web_mcp_import.rs`, `tests/serve_runs_api.rs`, `tc_2513`, `tc_2514`.
Rollback: `[web] require_token = true`, `[web] trust_wsl_host = false`.

## 7. Exit criteria
All TCs pass, gate and web tests pass, benchmark within 10%.
