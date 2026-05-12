# Native Rust Hello World

This example shows the Rust-native Agentic Harness API. Agents are regular Rust
handlers registered in `src/main.rs`; the binary embeds Agentic Harness's
manifest, run, and serve commands. The `assistant` agent demonstrates the
model-agnostic session API, so providers can be plugged in from Rust without
changing Agentic Harness's handler shape. The workspace includes `AGENTS.md`,
optional `CLAUDE.md`, a role, and native shell/file helpers to mirror the
conventions users expect from Claude Code and Codex-style agent projects.

## Run The Embedded Agent Binary

```bash
cargo run --manifest-path examples/hello-world/Cargo.toml -- \
  --agentic-harness-manifest
cargo run --manifest-path examples/hello-world/Cargo.toml -- \
  --agentic-harness-run hello --id demo --payload '{"name":"Ada"}'
```

## Run Through The Workspace CLI

```bash
cargo run -p agentic-harness-cli -- run hello \
  --workspace examples/hello-world \
  --id demo \
  --payload '{"name":"Ada"}'
```

## Serve Locally

```bash
cargo run -p agentic-harness-cli -- dev \
  --workspace examples/hello-world \
  --port 3583
```

Then call the HTTP route:

```bash
curl http://127.0.0.1:3583/agents/hello/demo \
  -H "Content-Type: application/json" \
  -d '{"name":"Ada"}'
```
