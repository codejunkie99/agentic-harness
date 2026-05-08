# Changelog

## 0.1.0

- Native Rust SDK and CLI for defining agents, loading workspace context,
  serving HTTP handlers, and running local agent workflows.
- The SDK now exposes `agentic_harness::prelude::*` for the common Rust agent
  authoring surface used by examples and docs.
- `agentic-harness code` starts the coding-agent flow, inspects a repository,
  can launch a selected coding LLM CLI with `--llm`, runs checks, writes
  latest-run summaries, supports `inspect`, and can commit or create a PR after
  success. Markdown and JSON summaries now include a coding loop timeline for
  inspect, plan, edit, test, summarize, commit, and pull-request phases.
- `--llm auto` detects installed coding CLIs in the Claude Code, Codex, Cursor,
  then Wind Server order before falling back to Codex.
- `doctor --json` and `smoke --json` expose which supported coding CLIs are
  installed and ready before users run `code --llm auto`, separating missing
  binaries from auth/keychain blockers.
- LLM readiness probes tolerate slower CLI startup under integration-test load
  so `template author --open` does not incorrectly report ready fake or real
  CLIs as timed out.
- Codex and Claude Code launcher paths now use their real non-interactive
  coding-agent modes instead of passing only the brief file path.
- Cursor launcher paths use `cursor agent --print --trust --force` for
  headless authoring and coding runs.
- Live LLM launcher output is streamed while still being captured for run
  summaries, so long Codex/Claude authoring and coding runs do not appear
  frozen.
- `template author --open` validates the generated `./<template>` pack after
  the selected LLM exits and reports the template path before returning.
- `template author --open --json` now preserves stdout as a single final JSON
  object and reports `opened` plus `postOpenValidation` while streaming live LLM
  output to stderr.
- Coding-agent checks run through the configured remote sandbox `SessionEnv`
  when the workspace sandbox target is not local, with a workspace sync before
  the remote check phase.
- Coding-agent Markdown and JSON summaries include next commands for inspecting
  the result, opening the dashboard, and rerunning the loop from the same
  workspace.
- Coding-agent summaries include sandbox target, endpoint, and local-vs-remote
  check mode so downstream agents can tell where verification actually ran.
- Coding-agent summaries expose parsed JSON `agentResult` separately from raw
  stdout, including fields such as `summary` and `generatedPatch`.
- Coding-agent summaries now record files edited by an external coding LLM
  under `llm.changedFiles` in JSON and in the Markdown LLM section.
- `agentic-harness code` now uses an actionable default coding brief in
  non-interactive no-prompt runs instead of the vague `Inspect the project`
  fallback.
- Dashboard sandbox status now runs the configured sandbox smoke check, so
  remote HTTP SessionEnv endpoints report remote readiness instead of local
  checkout readiness.
- TUI-style wizard for coding, templates, LLM authoring setup, sandbox setup,
  running agents, and doctor checks, with `tui` and `ui` aliases for the common
  entrypoint. Second-level screens show contextual panels for LLM availability,
  latest results, template previews, authoring setup, sandbox status/logs,
  example manifests, readiness, and next fixes. Option screens preview the exact
  workspace-aware command before execution, contextual panel commands honor the
  selected `--workspace`, and the sandbox panel shows smoke status plus the
  latest log entry. The guided LLM coding shortcut
  uses `agentic-harness code --llm auto` so it follows local CLI detection.
  Dashboard and coding panels surface the latest coding-run loop status when a
  latest summary exists. `wizard`/`tui`/`ui` accept `--workspace <path>` so the
  dashboard panels and current-workspace actions target the selected project.
- Template packs, including `template init`, validation, install, preview, and
  `new --template` scaffolding, plus `template author --open` for preflighting
  LLM readiness before launching the selected tool from the target workspace
  with the generated brief and live output. Authoring briefs now include
  validate, JSON preview, install, scaffold, and doctor commands plus a local
  CLI fallback path for validation when
  `agentic-harness` is not on PATH, and `template author --json` exposes the
  brief path, open command, and validation commands for software agents and TUI
  panels. `template search` and `tpl find` search built-in, workspace, user, and
  team template scopes with JSON output for agents.
- Local sandbox setup and local sandbox file/shell operations, plus remote
  connector instructions for hosted sandbox providers.
- Release packaging starter artifacts: local install script, Homebrew formula,
  binary package staging with manifest/checksums, release-check command with
  Homebrew Ruby syntax validation, and clean-machine smoke-test checklist.
