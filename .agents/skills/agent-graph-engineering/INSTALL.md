# agent-graph-engineering (manual install)

`scripts/install.sh` from `donggrri/cursor-agent-graph-engineering` could not be run in the
analysis environment: HTTPS and SSH clone returned "Repository not found" (no token access
to the private repository).

These paths were added manually to match the expected install layout so this analysis
could follow the graph-engineering workflow. Replace with an official `install.sh` run
when repository access is available.

Expected upstream paths:

- `.agents/skills/agent-graph-engineering/`
- `.cursor/agents/graph-*.md`
- `.cursor/rules/agent-graph-engineering.mdc`
