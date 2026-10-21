# REVIEW: Open another workspace dashboard

FR-876: `POST /api/workspace/open` reuses a live discovery URL and otherwise spawns `--web -C <path> --port 0`. It does not set switch-pending and does not look at the current turn. The dialog button "Open in new window" opens that URL. Switch still returns 409 while a turn or graph run is in progress.

`cargo test --lib workspace_open_reuses_live_dashboard_and_spawns` and `workspace_dir_assets` passed.

The running installed binary does not include this until it is rebuilt.
