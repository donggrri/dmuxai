# PLAN: Open another workspace dashboard

Spawn `current_exe --web -C <path> --port 0` from `POST /api/workspace/open`. Reuse `<path>/.dmuxai/web.json` when its pid is alive. The dialog button calls that route and opens the URL. `switch` is unchanged.
