# DmuxAI UI screenshots

Reference captures of the Ratatui interface (rendered via `TestBackend`, not a live terminal).

| File | Description |
| --- | --- |
| `main-tui.png` | Default chat, prompt, and status bar |
| `agent-turn.png` | Active turn with assistant streaming text |
| `model-picker.png` | Model selection overlay (`F2`) |
| `mode-picker.png` | Ask / Plan / Implement picker (`/mode`) |

Regenerate:

```bash
DMUXAI_EXPORT_SCREENSHOTS=1 cargo test export_documentation_screenshots -- --nocapture
```
