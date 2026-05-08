# Virtual Sandbox

`VirtualSessionEnv` is a hostless sandbox for lightweight agents that need a
filesystem without exposing the local checkout.

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
