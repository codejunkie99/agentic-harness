# agentic-harness-cli

Native Rust CLI for creating, running, inspecting, and packaging Agentic Harness
workspaces.

The CLI stays Cargo-native: it invokes Rust workspaces directly instead of
bundling JavaScript or a TypeScript runtime.

## Daily Workflow

```bash
agentic-harness guide --workspace .
agentic-harness doctor --workspace . --json
agentic-harness code --workspace . --llm auto --prompt "Fix the failing tests"
agentic-harness inspect --workspace .
```

Use `guide` for the shortest setup path, `doctor` for readiness, `code` for the
coding-agent loop, and `inspect` for the latest run summary.

## Command Map

- `agentic-harness new <path>` scaffolds a Rust Agentic Harness agent project.
- `agentic-harness start`, `wizard`, `tui`, or `ui` opens the guided local TUI.
- `agentic-harness guide` prints the setup-to-coding path.
- `agentic-harness doctor` checks workspace readiness; `check` is an alias.
- `agentic-harness dashboard` shows workspace status; `status` is an alias.
- `agentic-harness code` runs the coding-agent loop.
- `agentic-harness inspect` reads the latest coding-run summary.
- `agentic-harness template` manages reusable template packs; `templates` and
  `tpl` are aliases.
- `agentic-harness setup llm`, `setup sandbox`, and `setup hosting` install
  local prerequisites.
- `agentic-harness sandbox` runs configured sandbox file and shell operations.
- `agentic-harness hosting` inspects or starts local HTTP hosting.
- `agentic-harness dev --workspace <path> --port 3583` runs the Rust agent
  server in development and restarts it when workspace files change.
- `agentic-harness run <agent> --workspace <path> --id <id> --payload '<json>'`
  invokes one agent.
- `agentic-harness serve --workspace <path> --addr 127.0.0.1:3583` serves the
  Rust app over HTTP.
- `agentic-harness manifest --workspace <path> [--cloudflare]` prints generic
  agent metadata or Cloudflare Worker routing metadata.
- `agentic-harness add <model|sandbox|mcp> --print` prints native Rust connector
  instructions.
- `agentic-harness package`, `smoke`, and `release-check` cover release
  packaging and validation.

## Templates

Built-in starters are:

```text
hello, triage, data, coding, code-review, test-fixer, docs-writer,
refactor-agent, release-agent, repo-analyst, support
```

Create and run one:

```bash
agentic-harness new ./review-agent --template code-review
agentic-harness run code-review --workspace ./review-agent --id demo \
  --payload '{"prompt":"Review the current changes","checks":["cargo test"]}'
```

## Build Targets

Native build:

```bash
agentic-harness build --workspace <path> --target native --output <path>
```

Node host package:

```bash
agentic-harness build --workspace <path> --target node --output <path>
```

Cloudflare Worker boundary build:

```bash
agentic-harness build --target cloudflare \
  --workspace <path> \
  --output <path> \
  [--worker-app app.js] \
  [--worker-wasm app.wasm] \
  [--worker-wasm-crate <path>]
```

This writes non-proxy Worker boundary files: `_entry.js`, `wrangler.jsonc`,
`cloudflare-manifest.json`, `agentic_harness_worker.js`,
`agentic_harness_app.js`, `agentic_harness_app.d.ts`, and optionally
`agentic_harness_app.wasm`.

## Cloudflare Boundary

The Cloudflare build target is not a full deployment runtime yet. It emits the
Worker routing, Durable Object config, SQL session plumbing, and event response
shim, while handler execution still requires a Worker-compatible Rust/WASM app
adapter. Use `--worker-app <path>` to copy that adapter into
`dist/agentic_harness_app.js`; use `--worker-wasm <path>` to copy its companion
WASM module into `dist/agentic_harness_app.wasm`.
`dist/agentic_harness_app.d.ts` describes the adapter context and exports. When
`--worker-wasm` is used without `--worker-app`, the CLI generates a default JSON
ABI adapter for `agentic_harness_alloc`, `agentic_harness_invoke`,
`agentic_harness_last_result_len`, exported `memory`, and optional
`agentic_harness_init`. Use `--worker-wasm-crate <path>` to build that WASM from
a Rust crate with
`cargo build --release --target wasm32-unknown-unknown --no-default-features`.
