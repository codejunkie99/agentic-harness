# Native Rust Hello World

This example shows the Rust-native Agentic Harness API. Agents are regular Rust handlers
registered in `src/main.rs`; the binary embeds Agentic Harness's manifest, run, and serve
commands. The `assistant` agent demonstrates the model-agnostic session API, so
providers can be plugged in from Rust without changing Agentic Harness's handler shape.
The workspace includes `AGENTS.md`, optional `CLAUDE.md`, a role, and native
shell/file helpers to mirror the conventions users expect from Claude Code and
Codex-style agent projects.

```bash
cargo run --manifest-path examples/hello-world/Cargo.toml -- --agentic-harness-manifest
cargo run --manifest-path examples/hello-world/Cargo.toml -- \
  --agentic-harness-run hello --id demo --payload '{"name":"Ada"}'
```

Through the native CLI:

```bash
cargo run -p agentic-harness-cli -- run hello \
  --workspace examples/hello-world \
  --id demo \
  --payload '{"name":"Ada"}'
```
