# Agentic Harness

Agentic Harness is a native Rust agent runtime framework.

This repository is a native Rust agent runtime, not an adapter around a
TypeScript implementation. Agents are Rust binaries. The SDK, CLI, runtime,
HTTP dispatch, session layer, workspace context, tools, and examples are
implemented in Rust.

The main product direction is software agents that work on real repositories:
local development, CI, and remote Linux sandboxes such as Vercel Sandbox,
Daytona, or E2B. Cloudflare output exists for replacement parity and lightweight
edge control/webhook routing, but it is not the center of the coding-agent
runtime because Workers are not a natural place to run compilers, tests, package
installs, or long-lived shell work.

For the feature-by-feature replacement status, see
[`docs/replacement-audit.md`](docs/replacement-audit.md).
For the local/CI/sandbox/Cloudflare target split, see
[`docs/execution-targets.md`](docs/execution-targets.md).

## Workspace

| Path | Purpose |
| --- | --- |
| [`crates/agentic-harness`](crates/agentic-harness) | Rust SDK: agent registry, context, sessions, roles, skills, tools, HTTP serving |
| [`crates/agentic-harness-cli`](crates/agentic-harness-cli) | Rust CLI: `wizard`, `code`, `new`, `template`, `setup`, `doctor`, `build`, `dev`, `run`, `serve`, `manifest`, `add` |
| [`examples/hello-world`](examples/hello-world) | Native Rust example agent workspace |

## Quickstart

Install the local checkout:

```bash
./scripts/install.sh
export PATH="$HOME/.agentic-harness/bin:$PATH"
agentic-harness --version
```

Create a native Agentic Harness project:

```bash
cargo run -p agentic-harness-cli -- wizard --plain
cargo run -p agentic-harness-cli -- new ./my-agent --name my-agent --template hello
cargo run -p agentic-harness-cli -- doctor --workspace ./my-agent --plain
cargo run -p agentic-harness-cli -- run hello --workspace ./my-agent --id demo --payload '{"name":"Ada"}'
cargo run -p agentic-harness-cli -- code --workspace examples/hello-world --prompt "Inspect the project"
```

Run the included example:

```bash
cargo run -p agentic-harness-cli -- manifest --workspace examples/hello-world
cargo run -p agentic-harness-cli -- run hello --workspace examples/hello-world --id demo --payload '{"name":"Ada"}'
cargo run -p agentic-harness-cli -- build --workspace examples/hello-world --output ./build
```

Serve it over HTTP:

```bash
cargo run -p agentic-harness-cli -- dev --workspace examples/hello-world --port 3583
curl http://127.0.0.1:3583/agents/hello/demo \
  -H 'Content-Type: application/json' \
  -d '{"name":"Ada"}'
```

## Rust SDK

```rust
use agentic_harness::prelude::*;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct HelloPayload {
    name: Option<String>,
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
    Ok(AgentApp::new()
        .with_workspace(".")
        .load_workspace_context()?
        .agent(AgentDefinition::webhook("hello", |ctx: AgentContext| {
            let payload: HelloPayload = ctx.payload()?;
            let name = payload.name.unwrap_or_else(|| "World".to_string());
            Ok(json!({
                "id": ctx.id(),
                "message": format!("Hello, {name}!")
            }))
        })))
}

fn main() {
    let code = match app().and_then(run_cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("[agentic-harness] {err}");
            1
        }
    };
    std::process::exit(code);
}
```

The SDK provides:

- Typed payload extraction with `AgentContext::payload`.
- Webhook and CLI-only agents.
- `/health`, `/agents`, `/agents/<name>/<id>`, and
  `/agents/<name>/<id>/events` HTTP/SSE handling with agent, text, tool, turn,
  result, and idle events. `AgentApp::handle_request` also accepts a
  platform-neutral `HttpRequest`, so Worker-style adapters can select SSE with
  `Accept: text/event-stream` on `/agents/<name>/<id>`.
- `AgentApp::invoke_with_event_sink` streams runtime events to a caller-supplied
  sink as the handler runs, which is the core hook a non-native Worker adapter
  can connect to a platform stream. `RuntimeEvent::to_sse_frame` gives adapters
  the same SSE frame encoding used by the built-in HTTP path.
- `AgentApp::handle_worker_app_request` accepts the compact
  `WorkerAppRequest { agentName, id, payload }` shape used by generated WASM
  glue and returns `WorkerAppResponse` with status, result, error envelope, and
  runtime events.
- `agentic_harness_worker_app!(app_fn)` exports the Rust WASM ABI functions
  expected by the generated Worker adapter: `agentic_harness_alloc`,
  `agentic_harness_dealloc`, `agentic_harness_invoke`,
  `agentic_harness_last_result_len`, and `agentic_harness_init`.
- `OpenAiCompatibleModel::with_transport` lets non-native adapters provide
  their own chat-completions HTTP transport, such as a Worker `fetch` bridge,
  without enabling the native `reqwest` client.
- `AgentApp::cloudflare_worker_manifest` exposes the webhook-agent to Durable
  Object binding/class mapping that the Worker boundary build uses for Wrangler
  output, while validating lower-kebab-case agent names.
  `CloudflareWorkerManifest::wrangler_config` produces the tested Wrangler JSON
  fragment for those bindings and SQLite migrations, and
  `worker_entrypoint` emits a non-proxy Worker module that routes requests to
  those Durable Objects.
- In-memory named sessions keyed by request id, runtime-agnostic persistence
  through `SessionStore`, and optional native file-backed persistence with
  `AgentApp::file_session_store`.
- Persisted `SessionData` entries with message, compaction, and branch-summary
  records. `Session::compact_with_summary` injects a `[Context Summary]` while
  keeping recent context, `PromptOptions::compaction` can auto-summarize when a
  token budget is exceeded, and `append_branch_summary` carries child-task notes
  forward.
- Provider-neutral `ModelClient`, `AgentApp::model`, `default_model`, provider
  settings with env-key resolution, and configured `Session::prompt_with_options`.
- Workspace-discovered or file-loadable runtime config for provider defaults and
  OpenAI-compatible model registration. See
  [`docs/runtime-config.md`](docs/runtime-config.md).
- Built-in `OpenAiCompatibleModel` for OpenAI-compatible chat completion APIs
  using Rust-native HTTP.
- Structured result extraction from `---RESULT_START---` /
  `---RESULT_END---` blocks with `PromptResponse::result_json`, plus
  `PromptOptions::result_schema` to inject JSON-schema guidance before a typed
  `prompt_json_with_options` call.
- Detached child work with `Session::task` and `Session::task_with_id`.
- Claude Code/Codex-style context from `AGENTS.md`, optional `CLAUDE.md`,
  available skills, working directory, and directory listing.
- Workspace roles from `.agentic-harness/roles` or `roles`.
- Workspace skills from `.agents/skills` or `skills`.
- Native tools: `shell`, `shell_with_options`, `read`, bounded
  `read_with_options`, `write`, `edit`, `stat`, `readdir`, `exists`, `mkdir`,
  `rm`, regex `grep`, wildcard `glob`.
- Scoped shell execution with explicit env, cwd, and timeout controls.
- Custom local, empty, or remote session environments through `SessionEnv`,
  `MemorySessionEnv`, `HttpSessionEnv`, and
  `AgentContext::session_with_id_and_env`.
- Fallible session constructors such as `AgentContext::try_session_with_id_and_env`
  for runtimes that need storage load errors to abort the request instead of
  falling back to an empty history.
- Explicit host commands with `CommandDef` and `AgentContext::command`, so
  sensitive CLIs can be registered deliberately instead of exposing the whole
  host environment.
- Built-in model tools for `read`, `write`, `edit`, `bash`, `grep`, `glob`,
  and `task`, executed through the active session environment.
- Custom tools with `ToolDef`, JSON parameter schemas, duplicate-name checks,
  direct invocation, prompt-scoped tool metadata in `ModelRequest`, and
  model-requested tool execution through `ToolCall`.
- MCP tools from streamable HTTP servers or legacy SSE servers with
  `McpServerOptions::transport(McpTransport::Sse)`.

Automatic compaction keeps long-running Rust sessions inside a configured
context budget:

```rust
use agentic_harness::{CompactionSettings, PromptOptions};

let response = session.prompt_with_options(
    "Continue from the current plan.",
    PromptOptions::new().compaction(
        CompactionSettings::new()
            .context_window_tokens(128_000)
            .reserve_tokens(16_384)
            .keep_recent_messages(12),
    ),
)?;
```

Schema-guided results mirror the old `result: schema` examples while keeping the
schema and typed decode in Rust:

```rust
use agentic_harness::PromptOptions;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct TriageResult {
    approved: bool,
    comments: Vec<String>,
}

let result: TriageResult = session.prompt_json_with_options(
    "Review this change.",
    PromptOptions::new().result_schema(json!({
        "type": "object",
        "properties": {
            "approved": { "type": "boolean" },
            "comments": {
                "type": "array",
                "items": { "type": "string" }
            }
        },
        "required": ["approved", "comments"]
    })),
)?;
```

Use `MemorySessionEnv` when an agent needs an empty isolated filesystem instead
of the host workspace:

```rust
use agentic_harness::MemorySessionEnv;

let session = ctx.session_with_id_and_env(
    "isolated",
    MemorySessionEnv::new("/workspace"),
);
session.write("notes.txt", "private scratch data")?;
```

Use `HttpSessionEnv` when a sandbox provider exposes command and filesystem
operations over HTTP. Agentic Harness sends one JSON `POST` per operation with
an `op` field such as `exec`, `read`, `write`, `stat`, `readdir`, `exists`,
`mkdir`, or `rm`; binary writes use `contentBase64`. See
[`docs/http-session-env.md`](docs/http-session-env.md) for the exact protocol.

```rust
use agentic_harness::HttpSessionEnv;

let remote = HttpSessionEnv::new("https://sandbox.example/session", "/workspace")
    .header("Authorization", format!("Bearer {}", std::env::var("SANDBOX_TOKEN")?));

let session = ctx.session_with_id_and_env("remote", remote);
let output = session.shell("cargo test")?;
```

For coding agents, this remote-session shape is the important deployment
primitive: the agent stays native Rust, while shell/file operations run in the
selected local checkout or remote sandbox.

## Rust CLI

```bash
agentic-harness wizard|tui|ui [--workspace <path>] [--plain]
agentic-harness dashboard [--workspace <path>] [--plain] [--json]
agentic-harness status [--workspace <path>] [--plain] [--json]
agentic-harness guide [--workspace <path>] [--env claude-code|codex|cursor|wind-server|auto] [--template <name>] [--prompt <text>] [--json]
agentic-harness start [--workspace <path>] [--prompt <text>] [--id <id>] [--apply <patch>...] [--llm claude-code|codex|cursor|wind-server|auto] [--test <command>...] [--no-tests] [--commit <message>] [--pr] [--summary <path>] [--summary-json <path>]
agentic-harness code [--workspace <path>] [--prompt <text>] [--id <id>] [--apply <patch>...] [--llm claude-code|codex|cursor|wind-server|auto] [--test <command>...] [--no-tests] [--commit <message>] [--pr] [--summary <path>] [--summary-json <path>]
agentic-harness result|inspect|last [--workspace <path>] [--json]
agentic-harness new <path> [--name <package>] [--template hello|triage|data|coding|code-review|test-fixer|repo-analyst|support]
agentic-harness template init <path> [--name <name>] [--agent <agent>] [--description <text>] [--version <version>]
agentic-harness template author <name> [--workspace <path>] [--env claude-code|codex|cursor|wind-server|auto] [--prompt <text>] [--open] [--json]
agentic-harness template list [--workspace <path>] [--verbose] [--json]
agentic-harness template search <query> [--workspace <path>] [--json]
agentic-harness template show <name-or-path> [--workspace <path>] [--json]
agentic-harness template validate <path>
agentic-harness template export <name-or-path> [--workspace <path>] --output <path>
agentic-harness template install <path> [--workspace <path>] [--scope workspace|user|team] [--name <name>]
agentic-harness template import <path> [--workspace <path>] [--scope workspace|user|team] [--name <name>]
agentic-harness setup llm [--workspace <path>] [--env claude-code|codex|cursor|wind-server|auto] [--print]
agentic-harness setup sandbox [--workspace <path>] [--target local|vercel|daytona|e2b|custom] [--endpoint <url>] [--print]
agentic-harness sandbox status [--workspace <path>] [--json]
agentic-harness sandbox exec <command> [--workspace <path>] [--json]
agentic-harness sandbox read <path> [--workspace <path>]
agentic-harness sandbox write <path> --content <text> [--workspace <path>]
agentic-harness sandbox ls [path] [--workspace <path>]
agentic-harness sandbox sync <source> <destination> [--workspace <path>]
agentic-harness sandbox logs [--workspace <path>] [--json]
agentic-harness sandbox rm <path> [--recursive] [--workspace <path>]
agentic-harness doctor [--workspace <path>] [--plain] [--json]
agentic-harness check [--workspace <path>] [--plain] [--json]
agentic-harness smoke [--workspace <path>] [--json]
agentic-harness release-check [--root <path>] [--json]
agentic-harness package [--output <path>] [--binary <path>] [--json]
agentic-harness build --workspace <path> [--output <path>] [--target native|node|cloudflare] [--worker-app app.js] [--worker-wasm app.wasm] [--worker-wasm-crate <path>] [--env .env]
agentic-harness dev --workspace <path> [--port 3583] [--target native|node] [--env .env]
agentic-harness run <agent> --workspace <path> --id <id> [--payload '<json>'] [--target native|node] [--env .env]
agentic-harness serve --workspace <path> [--addr 127.0.0.1:3583] [--env .env]
agentic-harness manifest --workspace <path> [--cloudflare]
agentic-harness add [model|sandbox|mcp|daytona|vercel] [--print]
```

`agentic-harness guide` is the shortest start-here path. It prints the concrete
sequence from install to LLM setup, template authoring, coding, and result
inspection. `quickstart` and `onboard` are aliases. Use `--json` when a software
agent needs the same ordered steps without scraping terminal text.

`agentic-harness code` is intentionally short enough for the common path. If
you omit `--prompt` in a non-interactive run, the CLI gives the coding agent a
default brief to inspect the repository, plan the smallest safe coding step, run
available checks, and summarize the result.

Short template aliases are available for the common human path:

```bash
agentic-harness tpl create ./my-template --name my-template
agentic-harness templates ls --verbose
agentic-harness tpl preview coding
agentic-harness templates check ./my-template
agentic-harness tpl add ./my-template --scope user
agentic-harness tpl use ./exported-template --scope team
```

`agentic-harness wizard` is the runtime-local setup front door; `tui` and `ui`
are shorter aliases for the same flow. Use `--workspace <path>` to point the TUI
at a specific project. In an interactive terminal it opens with a status panel
for that workspace, readiness, sandbox target, template counts, recent logs, and
the next step, then shows the two-step numbered wizard: choose a workflow, then
select from multiple concrete actions inside that workflow. Second-level
workflow screens add context
panels for coding LLM availability, latest results, template JSON previews,
registry data, LLM authoring setup, sandbox status, sandbox logs, example
manifests, readiness, and next fixes. Each selectable option previews the exact
workspace-aware command before it runs, so users can choose by number without
copying a long command. Contextual panel commands also honor the selected
`--workspace`, so JSON previews, status checks, logs, and inspect links point at
the same project. The sandbox screen also shows smoke status and the latest
sandbox log entry. The start-coding screen includes both the plain local loop
and a detected-LLM option that runs
`agentic-harness code --workspace <path> --llm auto`. Current-workspace actions
run against the selected `--workspace` path, while other actions prompt for
missing values and run the same native CLI paths used by the regular commands.
Use `b` to go back and `q` to quit. In non-interactive shells, use
`agentic-harness wizard --workspace ./my-agent --plain` to print the same
status, workflow, and option map.
Template install and import actions prompt for `workspace`, `user`, or `team`
registry scope.

`agentic-harness dashboard` is the status surface for a workspace. It combines
doctor readiness, template registry metadata, sandbox target/endpoint smoke status,
template-authoring briefs, the latest coding-run loop status, recent sandbox
logs, and the next commands a coding agent should run. Use `--json` when
another software agent needs the same readiness, template, latest-run, sandbox,
log, and next-command data without scraping the TUI text. `agentic-harness
status` is the short alias for the same command.

`agentic-harness start` is the short start-coding path; `code` is the explicit
subcommand name for the same flow. It checks the workspace, warns if LLM
template-authoring context is missing, inspects the repository, reads workspace
instruction files such as `AGENTS.md` and `CLAUDE.md`, detects project manifest
files, captures git diff/name context, prepares deterministic `plannedSteps`,
runs the `code` agent with a prompt and plan, can apply one or more unified
diff patches with `--apply`, can apply a unified diff returned by the agent as
`generatedPatch`, and can launch a real coding LLM CLI with `--llm codex`,
`--llm claude-code`, `--llm cursor`, or `--llm wind-server`. The LLM path writes
a run-specific `.agentic-harness/runs/<id>/coding-brief.md`, invokes the
selected command in the workspace so it can edit files directly, then runs
either explicit `--test` commands or detected checks such as `cargo test`.
Codex runs through `codex exec` with the brief on stdin; Claude Code runs with
`claude -p --permission-mode acceptEdits`; Cursor runs with
`cursor agent --print --trust --force`.
When `.agentic-harness/sandbox.toml` points at a non-local sandbox endpoint,
the check phase first syncs the workspace into that remote `SessionEnv`, then
executes checks there instead of using local `sh -c`.
If checks fail, it makes one repair call with
`repairAttempt` and `failedChecks`; a repair `generatedPatch` is applied before
checks are rerun. Progress is written to stderr for each stage while stdout
remains available for the agent result JSON. Use `--commit "message"` to create
a git commit only after the agent, patch, and check phases succeed. Use `--pr`
to run `gh pr create --fill` only after the preceding phases succeed. When run
without custom summary flags, it saves `.agentic-harness/runs/latest.md` and
`.agentic-harness/runs/latest.json` for the latest run. Use
`agentic-harness inspect --workspace .` to read the Markdown report, or add
`--json` to read the structured report. Use `--summary <path>` or
`--summary-json <path>` when a specific artifact path is needed for CI or a
script.
Both summary formats include a first-class coding loop timeline with statuses
for inspect, plan, edit, test, summarize, commit, and pull-request phases so a
person or software agent can see exactly where the run stopped. They also
include sandbox target, endpoint, and local-vs-remote check mode, plus next
commands for inspecting the result, opening the dashboard, and rerunning the
coding loop from the same workspace. When the agent returns JSON, the summaries
also expose that parsed `agentResult` separately from raw stdout so downstream
tools can read fields such as `summary` and `generatedPatch` without scraping
terminal output. When an external coding LLM edits files directly, the JSON
summary also records those files under `llm.changedFiles`, and the Markdown
summary lists them in the LLM coding tool section.
Use `--no-tests` when you only want the inspection/agent step.

`agentic-harness template ...` manages reusable template packs with
`agentic-template.toml`, templated Rust files, roles, skills, and sample
payloads. `template init` creates a valid starter pack for an LLM to customize.
`template author` creates a workspace-local handoff brief under
`.agentic-harness/template-briefs/` for Claude Code, Codex, Cursor, or Wind
Server after `setup llm` has installed the authoring context. Add `--open` to
preflight the selected LLM readiness and then run it from that workspace with
the generated brief content, so the LLM can write the template pack directly
under `./<name>`; live LLM output streams while the command still captures
output for diagnostics. After the LLM exits, `--open` validates the generated
`./<name>` pack immediately and reports the template path, so users do not have
to guess whether the LLM produced a reusable pack. The generated brief also
asks the LLM to validate the pack, preview its manifest as JSON, install it,
scaffold a generated agent, and run doctor on that agent. It includes the
current local CLI path as a fallback when
`agentic-harness` is not on PATH inside the authoring workspace, and it tells the
LLM to leave the Agentic Harness SDK/CLI crates alone while producing the
template pack.
Add `--json` when a software agent or TUI needs the brief path, open command,
validation commands, next scaffold command, and generated-agent doctor check as
structured data. With `--open --json`, live LLM output is echoed to stderr so
stdout stays one parseable JSON object; the final JSON includes `opened` and
`postOpenValidation` fields after the generated pack is checked.
Template manifests carry a version, and `template list --verbose` shows version,
agent, and description metadata for built-in, workspace, user, and team packs.
Use `template search <query>` to find built-ins or installed packs by name,
agent, version, source, or description. Use `template list --json` and
`template search --json` when a software agent or TUI panel needs the same
registry as structured data with source scopes, versions, validity, and counts.
`template show --json` exposes one template preview with metadata, files,
validity, and the next scaffold command. `template show` previews built-in,
scoped, or path-based templates before scaffolding. `template export` copies a pack to a shareable folder and
`template import --scope user|team` installs an exported pack into a scoped
workspace registry. Workspace packs live under `.agentic-harness/templates`,
user packs under `.agentic-harness/templates/user`, and team packs under
`.agentic-harness/templates/team`.

`agentic-harness setup llm` installs template-authoring instructions for Claude
Code, Codex, Cursor, or Wind Server so an LLM can create reusable templates
instead of one-off SDK snippets. It also installs a small valid reference pack
at `.agentic-harness/template-examples/code-review` so the LLM can copy the
manifest, templated Rust entrypoint, roles, skills, and payload shape before
running `agentic-harness template validate`. The `auto` environment probes the
PATH in order for `claude`, `codex`, `cursor`, and `wind-server`; if none are
available it falls back to Codex so the printed commands stay concrete.

`agentic-harness setup sandbox` configures where code and shell work run. The
local checkout needs no credentials; named remote targets such as Vercel
Sandbox, Daytona, and E2B print or install `SessionEnv` connector instructions.
If a remote target exposes the documented HTTP SessionEnv protocol, pass
`--endpoint <url>` and the CLI sandbox commands will use that endpoint for
command and file operations.

`agentic-harness sandbox status --json` reports target, cwd, endpoint,
capabilities, smoke result, and recent logs for software agents and TUI panels.
For configured HTTP SessionEnv endpoints, status runs a harmless `pwd` and
directory-list smoke check through the remote sandbox.
`agentic-harness sandbox exec --json` captures exit code, stdout, and stderr in
a single structured object for coding agents.
`agentic-harness sandbox logs --json` returns ordered log entries with their
workspace and log path for richer TUI log panels.

`agentic-harness sandbox ...` performs local sandbox operations after setup:
status, shell execution, file reads, file writes, directory listing, host-to-
sandbox sync, operation logs, and cleanup. Remote targets intentionally route
through generated `SessionEnv` connector instructions until a project wires its
provider-specific connector or configures an HTTP endpoint.

`agentic-harness doctor` checks the workspace before the user or a software
agent tries to run it. It validates `Cargo.toml`, `src/main.rs`, `AGENTS.md`,
roles, skills, sandbox readiness, LLM template-authoring setup, and whether
Claude Code, Codex, Cursor, or Wind Server CLIs are installed and ready for
`code --llm auto`; auth/keychain failures are reported separately from missing
binaries. It then prints the next command when required files are missing.
`agentic-harness check` is the short alias for the same readiness check. Use
`--json` to return the same required and optional checks, fixes, readiness, and
next command as structured data for software agents.

`agentic-harness smoke --json` is the one-command post-install check. It
validates the installed CLI version, plain wizard rendering, doctor readiness,
LLM tool readiness, and sandbox smoke status for the selected workspace.
`agentic-harness release-check --json` is the pre-publish packaging check. It
verifies the local install script, Homebrew formula metadata and Ruby syntax,
binary package docs, changelog, and release smoke checklist before a tag or
release artifact is cut.
`agentic-harness package --output dist/packages --json` stages the current CLI
binary into a versioned OS/architecture folder with `manifest.json` and
`SHA256SUMS` so release jobs have a concrete binary artifact to upload.

`agentic-harness new --template ...` creates a native Rust starter for common
agent shapes: `hello`, CLI-only `triage`, workspace-aware `data`, repository
`coding`, `code-review`, `test-fixer`, `repo-analyst`, and customer `support`.
Each template includes a Rust handler, `AGENTS.md`, a role, and a starter skill
so Claude Code, Codex, or another coding agent can continue wiring the harness
from concrete files.

## Release Packaging

Distribution artifacts live in:

- [`scripts/install.sh`](scripts/install.sh) for local checkout or tagged Git
  installs.
- [`Formula/agentic-harness.rb`](Formula/agentic-harness.rb) for Homebrew HEAD
  installs.
- [`CHANGELOG.md`](CHANGELOG.md) for release notes.
- [`docs/release-smoke-test.md`](docs/release-smoke-test.md) for clean-machine
  release checks, including `agentic-harness smoke --json` and
  `agentic-harness release-check --json`.
- `agentic-harness package --output dist/packages --json` for local binary
  package staging with `manifest.json` and `SHA256SUMS`.

`agentic-harness dev` starts the native HTTP server and watches the workspace for
changes. Source/config changes restart the child server; generated directories
such as `target`, `dist`, `.git`, and `node_modules` are ignored.

`agentic-harness build` with `--target native` or `--target node` produces:

```text
dist/
  agentic-harness-agent
  manifest.json
```

The built binary is self-contained except for the normal dynamic libraries of
the Rust target platform. It embeds the Agentic Harness manifest/run/serve entry
points.

`agentic-harness build --target cloudflare` produces non-proxy Worker boundary
artifacts:

```text
dist/
  _entry.js
  agentic_harness_app.d.ts
  agentic_harness_app.js
  agentic_harness_worker.js
  cloudflare-manifest.json
  wrangler.jsonc
```

`_entry.js` routes `/health`, `/agents`, and agent requests to Durable Object
bindings generated from the Rust app manifest. `wrangler.jsonc` contains those
Durable Object bindings and SQLite migrations. `agentic_harness_worker.js`
contains the Worker-side JSON parsing, SSE, webhook, and Durable Object SQL
session plumbing. `agentic_harness_app.js` is the remaining app-adapter boundary
and returns `handler_not_linked` until replaced by Rust/WASM handler linkage; it
does not proxy to a native process. `agentic_harness_app.d.ts` documents the
stable adapter context, session store, event emitter, and exports.

Pass `--worker-app <path>` with `--target cloudflare` to copy a
Worker-compatible adapter into `dist/agentic_harness_app.js` during the build.
That adapter must export `initAgenticHarnessApp()` and
`invokeAgenticHarnessAgent(context)`.
Pass `--worker-wasm <path>` to also copy the companion WASM module to
`dist/agentic_harness_app.wasm`. If `--worker-wasm` is provided without
`--worker-app`, the CLI generates a default JSON ABI adapter that expects
`agentic_harness_alloc`, `agentic_harness_invoke`,
`agentic_harness_last_result_len`, exported `memory`, and optional
`agentic_harness_init`. The WASM side should return the serialized
`WorkerAppResponse`; the generated adapter throws typed errors and returns the
unwrapped `result`.
Pass `--worker-wasm-crate <path>` to compile a Rust adapter crate with
`cargo build --release --target wasm32-unknown-unknown --no-default-features`
and package the resulting module as `dist/agentic_harness_app.wasm`.

The SDK defaults to the `native` feature for local binaries. The provider-neutral
core also checks with `cargo check -p agentic-harness --no-default-features`,
which keeps future non-native runtimes from depending on the native blocking
HTTP client, filesystem/process helpers, TCP server, file-backed session store,
or embedded CLI by accident. Non-native runtimes can provide their own
`SessionStore` implementation for persisted `SessionData`, their own
`HttpModelTransport` for model calls through platform HTTP, and their own
`HttpSessionTransport` for remote sandbox operations.

`--target node` is accepted as a compatibility alias for existing Node-target
scripts, but it still produces/runs the native Rust server artifact.
`--target cloudflare` is currently build-only: `run` and `dev` still reject it
because Workers are only an edge/control adapter in the current design, not the
primary environment for coding work. `agentic-harness manifest --cloudflare`
prints the Worker routing metadata used by the build output. See
[`docs/cloudflare-runtime.md`](docs/cloudflare-runtime.md).

`agentic-harness add <connector>` supports Claude Code, Codex, Cursor, and
similar coding agents: human runs get a short pipeable recipe, while `--print`
emits agent-readable Rust connector instructions.
Known coding-agent environments are detected automatically, so agent-driven
runs receive the raw connector instructions even when stdout is attached to a
terminal.
The native registry includes connector categories, provider URLs, and aliases
such as `@vercel/sandbox`, `sandbox-e2b`, and `e2b-sandbox`; lookup is
case-insensitive for slugs and aliases.
When stdout is piped, `add` prints those raw instructions automatically, so both
forms work:

```bash
agentic-harness add daytona | codex
agentic-harness add e2b | claude
agentic-harness add https://e2b.dev/docs --category sandbox | claude
```

## Improvements Over The TypeScript Version

- No Node.js runtime, pnpm workspace, JS bundling step, or generated server
  source is required.
- The built artifact is a native executable plus a manifest.
- Handler APIs are typed at the Rust boundary instead of relying on generated
  TypeScript bundles.
- Model integration is a trait (`ModelClient`), so providers can be swapped
  without changing agent handlers.
- File/search/edit helpers are ordinary Rust methods and are easy to unit test.
- The test suite covers SDK behavior, CLI behavior, generated build artifacts,
  env-file loading, session persistence, and HTTP route behavior.

## Development

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```
