# agentic-harness-cli

Native Rust CLI for Agentic Harness.

Commands:

- `agentic-harness new <path>` scaffolds a Rust Agentic Harness agent project.
- `agentic-harness build --workspace <path> --output <path>` builds a release
  binary and writes `dist/manifest.json`.
- `agentic-harness build --target cloudflare --workspace <path> --output
  <path> [--worker-app app.js] [--worker-wasm app.wasm]
  [--worker-wasm-crate <path>]` writes non-proxy Worker boundary files:
  `_entry.js`, `wrangler.jsonc`, `cloudflare-manifest.json`,
  `agentic_harness_worker.js`, `agentic_harness_app.js`,
  `agentic_harness_app.d.ts`, and optionally `agentic_harness_app.wasm`.
- `agentic-harness dev --workspace <path> --port 3583` runs the Rust agent server in development and restarts it when workspace files change.
- `agentic-harness run <agent> --workspace <path> --id <id> --payload '<json>'` invokes one agent.
- `agentic-harness serve --workspace <path> --addr 127.0.0.1:3583` serves the Rust app over HTTP.
- `agentic-harness manifest --workspace <path> [--cloudflare]` prints generic
  agent metadata or Cloudflare Worker routing metadata.
- `agentic-harness add <model|sandbox|mcp> --print` prints native Rust connector instructions.

The CLI is intentionally Cargo-native: it invokes the Rust workspace directly
instead of bundling JavaScript.

The Cloudflare build target is not a full deployment runtime yet. It emits the
Worker routing, Durable Object config, SQL session plumbing, and event response
shim, while handler execution still requires a Worker-compatible Rust/WASM app
adapter.
Use `--worker-app <path>` to copy that adapter into
`dist/agentic_harness_app.js`; use `--worker-wasm <path>` to copy its companion
WASM module into `dist/agentic_harness_app.wasm`.
`dist/agentic_harness_app.d.ts` describes the adapter context and exports.
When `--worker-wasm` is used without `--worker-app`, the CLI generates a default
JSON ABI adapter for `agentic_harness_alloc`, `agentic_harness_invoke`,
`agentic_harness_last_result_len`, exported `memory`, and optional
`agentic_harness_init`.
Use `--worker-wasm-crate <path>` to build that WASM from a Rust crate with
`cargo build --release --target wasm32-unknown-unknown --no-default-features`.
