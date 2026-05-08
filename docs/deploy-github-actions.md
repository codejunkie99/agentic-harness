# Run in GitHub Actions

Use GitHub Actions for issue triage, PR review, test reproduction, and release
checks where the repository is already checked out.

```yaml
name: agentic-harness

on:
  issues:
    types: [opened]

jobs:
  triage:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      issues: write
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install --path crates/agentic-harness-cli
      - run: |
          agentic-harness run triage \
            --workspace . \
            --id "issue-${{ github.event.issue.number }}" \
            --payload '{"issueNumber":${{ github.event.issue.number }}}'
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

Register `gh`, `git`, or package-manager commands only from trusted Rust code
with `CommandDef` or a sandbox `SessionEnv`; do not put secrets in prompts.
