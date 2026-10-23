# Custom headless agents (`[[agents.custom]]`)

Any agent CLI with a headless (non-interactive) command can be added in `dmuxai.toml`, no Rust code.
It then shows in `/agent` and F2, gets its own chat color, and can be a graph runner via
`/graph override <node> <name> <model>`.

```toml
[[agents.custom]]
name = "opencode"                 # runner id: lowercase [a-z0-9_-]
label = "opencode"                # shown in /agent and the chat (default: name)
color = "#ffa500"                 # theme color (default: white)
command = ["opencode", "run", "--format", "json", "-m", "{model}", "{prompt}"]  # no --dir: the workspace is the child's cwd, which every opencode build honours
prompt_via = "arg"                # "arg" ({prompt} placeholder) or "stdin" (prompt written to stdin)
image_arg = ["-f", "{image}"]     # optional, repeated per image; else "@path" appended to the prompt
default_model = "opencode/glm-5.3-flash-free"
models_command = ["opencode", "models"]   # optional; one model id per stdout line
models_filter = "-free$"                  # optional regex on model ids
allowed_models = "^opencode/.*-free$"     # optional regex; model policy for this runner
output = "jsonl"                  # "text" (stdout is the reply) or "jsonl"
text_path = "/part/text"          # jsonl: JSON pointer of a text chunk
text_when = { path = "/type", equals = "text" }   # optional: only lines matching this
error_path = ["/error/data/message", "/error/message"]   # jsonl: JSON pointer(s) of an error message; a list is tried in order, and an `/error` object without a message string falls back to its compact JSON
session_id_path = "/sessionID"    # jsonl: JSON pointer of the provider session id
done_when = { path = "/type", equals = "end" }        # optional: this line ends the turn;
                                  # DmuxAI stops reading, waits 3 s, then stops the process group
text_break_when = { path = "/type", equals = "tool_call" }   # optional: next text starts a new paragraph
resume_arg = ["--session", "{session_id}"]   # appended when resuming
timeout_secs = 1800
failure_stderr = "(?i)permission denied|rejected permission"
temp_config_template = '{"permission":{"edit":"{edit}","bash":"{edit}","webfetch":"allow"}}'
env = { OPENCODE_CONFIG = "{temp_config}" }

[agents.custom.modes]             # extra args per mode (optional)
ask = []
plan = []
implement = []
```

Placeholders: `{prompt}`, `{model}`, `{workspace}`, `{image}`, `{session_id}`, `{temp_config}`, `{mode}`
(`ask|plan|implement`), `{edit}` (`allow` in Implement, else `deny`), `{system_prompt}` (resolved
prompt text), `{system_prompt_file}` (absolute temp file path, only when referenced).
A custom agent that references `{system_prompt}`/`{system_prompt_file}` or sets
`system_prompt_arg = [...]` opts into native delivery; otherwise the prompt is
prepended as `<agent-instructions>`.
Each argv element is substituted separately and passed directly to the process (no shell), so
prompt text can never inject arguments. An argument whose placeholder has no value
(e.g. `{model}` with no model chosen) is dropped together with the flag right before it
when that flag starts with `-`.

The default `timeout_secs` is 1800 (30 minutes); long implement turns often need a larger value.

When `output` is omitted but any JSONL path field (`text_path`, `text_when`, and the like) is set,
DmuxAI treats the agent as `output = "jsonl"`. If you set `output = "text"` explicitly while
those path fields remain, they are ignored and startup shows a config warning unless you switch
to `output = "jsonl"`. In text mode, if the first non-empty stdout line looks like a JSON object,
DmuxAI emits a one-time notice suggesting `output = "jsonl"` and `text_path`.

Failure (shown as an error turn): spawn error, non-zero exit, timeout, empty reply text,
a jsonl `error_path` hit, or stderr matching `failure_stderr`.
Process: own process group; Ctrl+C kills the whole group. The temp config is deleted after the turn.

Presets: see `docs/agents/presets.toml` (opencode tested; hermes tested; pi untested).

The built-in grok runner is this same preset shape: `grok -p` with
`--output-format streaming-json`, not a separate integration. `grok agent stdio`
is not the built-in path.
