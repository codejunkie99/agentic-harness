# Agentic Harness Immediate Goals

Status: implemented for the local CLI slice

This document defines the next product slice for Agentic Harness. The immediate
focus is local and sandboxed coding-agent creation, not deployment.

## Non-Goals For This Slice

- No hosting workflow.
- No deployment wizard.
- No production release automation.
- No Cloudflare, InsForge, or other deploy target work.
- No requirement that users hand-write Rust SDK code for every new agent shape.

## Product Principle

Agentic Harness should let users start from an operator flow, not from SDK
memorization. The CLI and TUI should install the right project context, then let
Claude Code, Codex, Cursor, or Wind Server help the user author reusable agent
templates.

The SDK remains the runtime foundation. Most users should reach it through
generated templates, template packs, and assisted setup.

For users who do write Rust directly, the SDK should expose a compact
`agentic_harness::prelude::*` import for the stable agent-authoring surface.

`agentic-harness guide` is the short start-here command for this flow. It prints
the ordered path from install to LLM setup, template authoring, start coding, and
result inspection; `--json` exposes the same steps to software agents.

## Immediate Goal 1: Sandbox Environment

Agentic Harness needs a first-class sandbox setup path for coding agents.
The first version should support:

- local checkout execution as the default sandbox,
- a remote sandbox connector path through `SessionEnv`,
- HTTP SessionEnv endpoint setup for remote command and file operations,
- clear sandbox status in `doctor`,
- a TUI setup step that asks where code should run,
- a smoke check that runs a harmless command in the selected environment.

The sandbox setup should answer:

- Where will the agent read and edit files?
- Where will shell commands run?
- What credentials are required?
- Can the environment run `pwd`, list files, and execute a test command?
- What should the user or agent fix if the check fails?

Acceptance criteria:

- `agentic-harness doctor` reports sandbox readiness.
- `agentic-harness doctor --json` exposes required checks, optional checks,
  fixes, readiness, and next command without requiring output scraping.
- The TUI has a `Set up sandbox` workflow.
- Local checkout works without credentials.
- Local sandbox operations can show status, execute a command, read files,
  write files, list directories, sync files, show logs, and clean up paths.
- `agentic-harness sandbox exec --json` exposes command, exit code, stdout, and
  stderr as structured output for agents and TUI panels.
- `agentic-harness sandbox status --json` exposes target, cwd, endpoint,
  capabilities, smoke result, and recent logs for agents and TUI panels.
  For configured HTTP SessionEnv endpoints, the smoke result comes from a
  remote `pwd` and directory-list probe rather than config presence alone.
- `agentic-harness sandbox logs --json` exposes ordered log entries for richer
  TUI log panels and software agents.
- Remote sandbox setup produces a connector file or clear agent instructions,
  including named Vercel Sandbox, Daytona, and E2B setup paths.
- Remote sandbox setup can store an endpoint and run `sandbox exec`, `read`,
  `write`, `ls`, and `rm` through the HTTP SessionEnv protocol.
- No deployment target is configured as part of this flow.

## Immediate Goal 2: Better TUI

The TUI should become the main human entrypoint. Long commands can remain for
automation, but humans should be able to use numbered screens.

Expected top-level shape:

```text
Agentic Harness

Project: ./my-agent
Status: Ready
Sandbox: Local checkout

1. Start coding
2. Create or install template
3. Set up LLM authoring environment
4. Set up sandbox
5. Run agent
6. Check setup
```

Interaction requirements:

- Pressing a top-level number opens a second screen with multiple concrete
  choices.
- Every screen supports `b` for back and `q` for quit.
- Every action shows the command it is about to run.
- Setup screens show status before asking for new input.
- Failed checks show the next corrective command.
- Deployment entries should not appear in this slice.

Acceptance criteria:

- `agentic-harness`, `agentic-harness wizard`, `agentic-harness tui`, or
  `agentic-harness ui` opens the TUI in a TTY.
- `agentic-harness wizard --workspace ./my-agent --plain`,
  `agentic-harness tui --workspace ./my-agent --plain`, and
  `agentic-harness ui --workspace ./my-agent --plain` print the same menu map
  for automation against the selected workspace.
- The wizard opens with a status panel for workspace readiness, sandbox target,
  template counts, recent sandbox logs, and the next step.
- Current-workspace wizard actions run against the selected `--workspace` path
  instead of always assuming `.`.
- Second-level selection screens include contextual panels for coding LLM
  availability, latest results, template JSON previews, registry data, LLM
  authoring setup, sandbox status, sandbox log views, example manifests,
  readiness, and next fixes.
- Second-level selection screens preview the exact workspace-aware command for
  each option before it runs, so users can choose by number instead of pasting a
  long command.
- The sandbox selection screen shows smoke status and the latest sandbox log
  entry, not only the configured target.
- `agentic-harness dashboard --workspace . --plain` shows readiness, templates,
  template-authoring briefs, the latest coding-run loop status, sandbox status,
  recent logs, and next commands.
- `agentic-harness dashboard --workspace . --json` exposes the same readiness,
  template, latest-run, sandbox, log, and next-command data for software
  agents. Sandbox smoke status comes from the configured sandbox target and
  endpoint, not a hard-coded local checkout probe.
- Short aliases `agentic-harness status` and `agentic-harness check` cover the
  common dashboard and doctor paths.
- The TUI includes template, LLM environment, sandbox, run, and doctor flows.
- The start-coding screen exposes a direct LLM-backed coding option such as
  `agentic-harness code --llm auto`.
- Template flows include a preview step before validation or installation.
- The TUI does not require users to paste long commands for the common path.

## Immediate Goal 3: One-Command Start Coding Flow

Users should be able to start from a template and run a coding agent with one
short command.

Target command:

```bash
agentic-harness code
```

The command should:

1. Detect the current project.
2. If no agent project exists, offer to create one from a coding template.
3. If no model or LLM authoring environment is configured, route to setup.
4. If no sandbox is configured, default to local checkout and offer remote setup.
5. Run `doctor`.
6. Inspect the repository status and root files.
7. Prepare deterministic plan steps from the prompt, repository context, patch
   inputs, and check commands.
8. Start the coding agent with a prompt collected from the user and the plan.
9. Apply prepared unified-diff patches when provided.
10. Apply a unified diff returned by the agent as `generatedPatch` when present.
11. Optionally launch a real coding LLM CLI with `--llm` and a generated
    `.agentic-harness/runs/<id>/coding-brief.md` so Codex, Claude Code, Cursor,
    or Wind Server can edit files directly before checks. Codex should use
    `codex exec` with the brief on stdin; Claude Code should use non-interactive
    print mode with edit permissions; Cursor should use
    `cursor agent --print --trust --force`.
12. Run explicit `--test` commands or detected local checks, syncing the
    workspace into the configured remote `SessionEnv` sandbox before the check
    phase when the sandbox target is not local.
13. If checks fail, call the coding agent once with `repairAttempt` and
    `failedChecks`, apply any repair `generatedPatch`, and rerun checks.
14. Optionally commit successful changes with an explicit message.
15. Optionally open a pull request with `gh pr create --fill`.
16. Write `.agentic-harness/runs/latest.md` and
    `.agentic-harness/runs/latest.json` when no custom summary path is provided.
17. Record a coding loop timeline with statuses for inspect, plan, edit, test,
    summarize, commit, and pull-request phases.
18. Include next commands for inspect, dashboard, and rerun in Markdown and JSON
    summaries.
19. Let the user inspect the latest result with `agentic-harness inspect`.

Non-interactive form:

```bash
agentic-harness code --workspace ./my-agent --prompt "Fix the failing tests"
agentic-harness inspect --workspace ./my-agent
agentic-harness code --workspace ./my-agent --prompt "Fix the failing tests" --llm codex --test "cargo test"
agentic-harness code --workspace ./my-agent --prompt "Fix the failing tests" --test "cargo test" --summary .agentic-harness/runs/last.md
agentic-harness code --workspace ./my-agent --prompt "Apply the patch" --apply ./change.patch --test "cargo test" --commit "Apply generated fix"
agentic-harness code --workspace ./my-agent --prompt "Open the PR" --apply ./change.patch --test "cargo test" --commit "Apply generated fix" --pr
```

Acceptance criteria:

- A fresh user can run one command and reach a working coding-agent loop.
- The coding template is the default for software work.
- The flow uses existing templates and setup checks instead of asking the user
  to manually wire the SDK.
- Non-interactive no-prompt runs use an actionable default coding brief instead
  of a vague inspection-only fallback.
- The loop passes repository inspection context and check commands into the
  coding agent.
- Repository inspection includes workspace instruction files such as `AGENTS.md`
  and `CLAUDE.md`, detected project manifest files, git diff stat, and git
  changed-file context.
- The loop passes `plannedSteps` into the coding agent and records the plan in
  the run summary.
- The loop can apply prepared patches before running checks.
- The loop can apply an agent-generated `generatedPatch` before running checks.
- The loop can launch the selected Claude Code, Codex, Cursor, or Wind Server
  CLI with a generated coding brief, let it edit the workspace directly, then
  run checks and record the LLM result in Markdown and JSON summaries.
- The loop syncs workspace files into a configured non-local sandbox before
  executing remote checks.
- The loop can pass failed check output back to the coding agent once, apply a
  repair patch, and rerun checks.
- The loop can create an explicit git commit after successful checks.
- The loop can open a pull request after successful checks and commit setup.
- The loop can execute checks and save Markdown or JSON run summaries for later
  review by humans or software agents, including default latest-run summaries
  for no-flag coding runs.
- `agentic-harness inspect --workspace <path>` reads the latest Markdown summary,
  and `--json` reads the latest structured summary.
- The run summary includes project context and a changed-files section derived
  from git status and applied patch metadata.
- The Markdown and JSON run summaries include a coding loop timeline that shows
  whether inspect, plan, edit, test, summarize, commit, and pull-request phases
  completed, skipped, failed, or produced an artifact.
- The Markdown and JSON run summaries include next commands for inspecting the
  result, opening the dashboard, and rerunning the loop from the same workspace.
- The Markdown and JSON run summaries include sandbox target, endpoint, and
  local-vs-remote check mode so agents can tell where verification ran.
- The Markdown and JSON run summaries expose files edited by an external coding
  LLM inside the LLM result, not only in the global changed-files section.
- The Markdown and JSON run summaries expose parsed JSON `agentResult`
  separately from raw stdout so agents can read fields such as `summary` and
  `generatedPatch` without scraping terminal output.
- The loop prints progress for inspect, plan, agent, patch, repair, check,
  commit, PR, and summary stages without corrupting stdout agent results.
- `agentic-harness doctor --json` and `agentic-harness smoke --json` expose
  Claude Code, Codex, Cursor, and Wind Server CLI availability and readiness
  before a user tries `agentic-harness code --llm auto`, including auth or
  keychain blockers when the binary exists but cannot run.
- Failures return actionable setup steps.

## Immediate Goal 4: LLM Environment Setup Before SDK Use

If someone wants an agent to use Agentic Harness, the first step should be
setting up their coding LLM environment. The LLM then helps create or refine the
agent template.

Supported environments:

- Claude Code
- Codex
- Cursor
- Wind Server

Target command:

```bash
agentic-harness setup llm
agentic-harness template author code-review --env codex --prompt "Create a code-review agent template" --open
agentic-harness template author code-review --env codex --prompt "Create a code-review agent template" --json
```

Expected sub-options:

```text
1. Claude Code
2. Codex
3. Cursor
4. Wind Server
5. Detect automatically
```

`auto` should probe the local PATH for `claude`, `codex`, `cursor`, and
`wind-server` in that order, then fall back to Codex if none are installed.

The setup should install or print:

- project instructions for Agentic Harness template authoring,
- a template-authoring skill under `.agents/skills`,
- examples of valid template packs,
- validation commands,
- the expected generated project shape.

After setup, a user should be able to ask their coding LLM:

```text
Create an Agentic Harness template for a code-review agent that can inspect a
repository, run tests, summarize risks, and return a structured review result.
```

The LLM should know how to create:

- `agentic-template.toml`,
- templated Rust source files,
- `AGENTS.md`,
- roles,
- skills,
- sample payloads,
- validation instructions.

Acceptance criteria:

- `agentic-harness setup llm` can install instructions for each supported LLM
  environment.
- The installed instructions teach the LLM to create reusable templates, not
  one-off SDK snippets.
- The installed authoring context includes at least one valid reference template
  pack under `.agentic-harness/template-examples/` that passes
  `agentic-harness template validate`.
- `agentic-harness doctor` can detect whether the LLM authoring environment is
  installed.
- `agentic-harness template author` creates an LLM handoff brief that points to
  the installed authoring skill, lists the required validate, preview, install,
  scaffold, and doctor commands, and includes the local CLI path as a fallback
  when `agentic-harness` is not on PATH. The brief explicitly tells the LLM to
  create a template pack without editing Agentic Harness SDK or CLI crates.
- `agentic-harness template author --json` exposes the brief path, selected LLM
  open command, validation commands, next scaffold command, and generated-agent
  doctor check as structured data for software agents and TUI panels.
- `agentic-harness template author --open` runs the selected Claude Code,
  Codex, Cursor, or Wind Server command from the target workspace only after
  preflighting LLM readiness, then streams live output while the LLM creates
  `./<template>` from the generated brief.
- After `--open` returns, the CLI validates the generated `./<template>` pack
  and reports the template path so users can immediately install or scaffold it.
- `agentic-harness template author --open --json` keeps stdout parseable for
  software agents by streaming live LLM output to stderr and returning final
  JSON with `opened` and `postOpenValidation` fields.
- Users can create new templates without opening SDK docs first.

## Immediate Goal 5: User-Owned Template Packs

Templates should be installable, inspectable, and reusable. Built-in templates
are only the starting point; the default library should include practical
software-agent shapes such as coding, code review, test repair, repository
analysis, triage, data, and support agents.

Proposed template shape:

```text
template-name/
  agentic-template.toml
  files/
    Cargo.toml.hbs
    src/main.rs.hbs
    AGENTS.md.hbs
    .agentic-harness/roles/coder.md
    .agents/skills/coding/SKILL.md
  examples/
    payload.json
  prompts/
    authoring-notes.md
```

Target commands:

```bash
agentic-harness template list
agentic-harness templates ls --verbose
agentic-harness templates ls --json
agentic-harness template install ./template-name --scope user
agentic-harness tpl add ./template-name --scope user
agentic-harness template import ./template-name --scope team
agentic-harness template validate ./template-name
agentic-harness templates check ./template-name
agentic-harness new ./my-agent --template template-name
```

Acceptance criteria:

- Templates can be loaded from built-ins or local folders.
- Templates can be imported into and exported from the local workspace registry.
- Templates can be installed into workspace, user, or team scopes and scaffolded
  back by name from each scope.
- Short aliases such as `tpl create`, `templates ls`, `tpl preview`, and
  `templates check` work for the common human path.
- Template packs carry version metadata that appears in verbose registry lists.
- Template packs are available through `template list --json` with source scope,
  version, validity, and registry counts for agents and future TUI panels.
- Template packs are searchable with `template search` and `tpl find` across
  built-in, workspace, user, and team scopes, with JSON output for agents.
- Individual templates are available through `template show --json` with
  metadata, files, validity, and the next scaffold command for preview panels.
- Template validation catches missing manifest, missing files, invalid package
  names, and missing sample payloads.
- The TUI can create, install, validate, and use templates, including choosing
  workspace, user, or team registry scope for install/import flows.
- LLM-authored templates are normal template packs that work without the LLM
  after they are created.

## Priority Order

1. Define template pack format and validation.
2. Add LLM environment setup for Claude Code, Codex, Cursor, and Wind Server.
3. Add sandbox setup and sandbox readiness checks.
4. Upgrade the TUI around template, LLM setup, sandbox, run, and doctor flows.
5. Add `agentic-harness code` as the one-command start coding path.
6. Add `agentic-harness guide` as the one-command start-here path.
7. Add distribution basics: version output, a local install script, a Homebrew
   formula, binary package staging, changelog, one-command
   `agentic-harness smoke --json`, and clean-machine smoke-test checklist.

## Definition Of Done

This slice is complete when a new user can:

1. Run `agentic-harness guide` or the TUI.
2. Set up their coding LLM environment.
3. Ask that LLM to create a custom Agentic Harness template.
4. Validate and install the template.
5. Start a coding agent from that template.
6. Inspect the result through dashboard/log output.
7. Run the agent locally or in a configured sandbox.

No deployment path is required for this definition of done.
Distribution packaging is allowed because it installs the local tool; it does
not add an application deployment path.
