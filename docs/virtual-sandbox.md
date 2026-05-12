# Virtual Sandbox

`VirtualSessionEnv` is a hostless sandbox for lightweight agents that need a
filesystem without exposing the local checkout.

Use it for tests, examples, and constrained support agents. Do not use it when
the agent needs a real compiler, package manager, browser, networked service, or
large filesystem.

```rust
use agentic_harness::{SessionEnv, ShellOptions, VirtualSessionEnv};

let env = VirtualSessionEnv::new("/workspace")
    .with_file("knowledge/intro.md", "Agentic Harness is native Rust.")?;

let result = env.exec("grep Rust knowledge/intro.md", ShellOptions::new())?;
assert_eq!(result.stdout, "Agentic Harness is native Rust.\n");
```

It supports in-memory file operations plus a small command set: `pwd`, `ls`,
`cat`, `echo >`, `mkdir`, `rm`, and `grep`. It is useful for support agents,
knowledge-base tests, and Worker-style adapters that should not touch host
files.

For production storage such as R2, S3, or a database, implement `SessionEnv` or
bridge through `HttpSessionEnv` so reads and writes go to that backing store.

## Good Fits

- unit tests for agents that read and write a few files,
- examples that should not mutate the host checkout,
- support or knowledge-base agents with small static inputs,
- Worker-style adapters that need a synchronous `SessionEnv` shape in tests.

## Poor Fits

- repository-wide coding tasks,
- commands that require Cargo, npm, Python, or system binaries beyond the small
  supported set,
- workloads that need durable storage after the process exits.
