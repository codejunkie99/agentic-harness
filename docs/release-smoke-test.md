# Release Smoke Test

Run this from a clean machine or a fresh user account before publishing a
release.

The goal is to verify the installed CLI, package staging, Homebrew formula,
runtime scaffolding, and the coding-agent loop from the same commands a user
would run.

## Local Checkout Install

```sh
git clone https://github.com/codejunkie99/agentic-harness.git
cd agentic-harness
./scripts/install.sh
export PATH="$HOME/.agentic-harness/bin:$PATH"
agentic-harness --version
agentic-harness package --output dist/packages --json
agentic-harness smoke --json
agentic-harness release-check --json
```

## Homebrew Formula Check

```sh
ruby -c Formula/agentic-harness.rb
brew install --HEAD ./Formula/agentic-harness.rb
agentic-harness --version
agentic-harness tui --plain
```

## Runtime Smoke

```sh
agentic-harness new /tmp/agentic-harness-smoke \
  --name agentic-harness-smoke \
  --template coding
agentic-harness tui --workspace /tmp/agentic-harness-smoke --plain
agentic-harness setup llm --workspace /tmp/agentic-harness-smoke --env codex
agentic-harness template validate \
  /tmp/agentic-harness-smoke/.agentic-harness/template-examples/code-review
agentic-harness setup sandbox --workspace /tmp/agentic-harness-smoke --target local
agentic-harness doctor --workspace /tmp/agentic-harness-smoke --plain
agentic-harness smoke --workspace /tmp/agentic-harness-smoke --json
agentic-harness code --workspace /tmp/agentic-harness-smoke --no-tests
agentic-harness inspect --workspace /tmp/agentic-harness-smoke
```

## Repository Checks

```sh
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
agentic-harness package --output dist/packages --json
```

## Passing Criteria

- `agentic-harness --version` prints the release version.
- `smoke --json` and `release-check --json` return success.
- The Homebrew formula installs the same CLI version.
- The runtime smoke project can be scaffolded, checked, and inspected.
- `cargo fmt`, `cargo test`, and `cargo clippy` pass for the repository.
