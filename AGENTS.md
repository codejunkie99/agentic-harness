# Agentic Harness Rust Runtime

This repository is the native Rust Agentic Harness runtime. It is implemented
directly in Rust without wrapping a TypeScript runtime.

## Project Structure

- `crates/agentic-harness/` — Rust SDK: agent app, context, sessions, tools, roles, skills, HTTP handling.
- `crates/agentic-harness-cli/` — Rust CLI: `new`, `build`, `dev`, `run`, `serve`, `manifest`, `add`.
- `examples/hello-world/` — Rust example workspace.

## Build And Test

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Running The Example

```bash
cargo run -p agentic-harness-cli -- manifest --workspace examples/hello-world
cargo run -p agentic-harness-cli -- run hello --workspace examples/hello-world --id demo --payload '{"name":"Ada"}'
cargo run -p agentic-harness-cli -- build --workspace examples/hello-world --output ./build
```

## Development Notes

- Do not add TypeScript packages, pnpm workspace files, or Node build steps.
- Keep the Rust SDK and Rust CLI as the primary implementation.
- Prefer tests in `crates/agentic-harness/tests/` and `crates/agentic-harness-cli/tests/`.
- Favor native Rust APIs and direct binaries over generated JavaScript.
