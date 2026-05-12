# HttpSessionEnv Protocol

`HttpSessionEnv` adapts a remote sandbox service to the `SessionEnv` trait. It
sends one JSON `POST` to the configured endpoint for each operation.

Native binaries can use `HttpSessionEnv::new(...)`, which uses the built-in HTTP
client. Non-native adapters can use `HttpSessionEnv::with_transport(...)` and
provide an `HttpSessionTransport` backed by platform HTTP, such as Worker
`fetch`.

For named hosted providers that already expose this protocol, prefer
`SandboxConnector::vercel(...)`, `SandboxConnector::daytona(...)`, or
`SandboxConnector::e2b(...)`. Those helpers tag requests with the provider name
and return an `HttpSessionEnv`.

## Requests

Every request contains an `op` field:

```json
{ "op": "read", "path": "src/main.rs" }
```

Supported operations:

- `exec`: sends `command`, `cwd`, `env`, and optional `timeoutMs`; returns
  `ShellOutput` JSON with `stdout`, `stderr`, and `exitCode`.
- `read`: sends `path`; returns a JSON string.
- `write`: sends `path` plus either UTF-8 `content` or `contentBase64`; returns
  any JSON value.
- `stat`: sends `path`; returns `FileStat` JSON with `isFile`, `isDirectory`,
  `isSymbolicLink`, `size`, and `modifiedUnixMs`.
- `readdir`: sends `path`; returns a JSON array of entry names.
- `exists`: sends `path`; returns a JSON boolean.
- `mkdir`: sends `path`; returns any JSON value.
- `rm`: sends `path` and `recursive`; returns any JSON value.

All paths are interpreted relative to the sandbox root configured when the
`HttpSessionEnv` is constructed. The server side should reject path traversal
outside that root.

For text writes, Agentic Harness sends:

```json
{ "op": "write", "path": "notes.txt", "content": "alpha" }
```

For binary writes, it sends:

```json
{ "op": "write", "path": "bin/blob.dat", "contentBase64": "/wAB" }
```

For command execution, it sends:

```json
{
  "op": "exec",
  "command": "cargo test",
  "cwd": "/workspace/project",
  "env": { "RUST_LOG": "info" },
  "timeoutMs": 120000
}
```

The corresponding success response should use this shape:

```json
{
  "stdout": "test result: ok\n",
  "stderr": "",
  "exitCode": 0
}
```

## Errors

Any non-2xx response is converted into an `AgenticHarnessError::Handler`. If the
response body contains `{ "error": { "message": "..." } }` or
`{ "message": "..." }`, that message is included in the error.

## Auth

Pass provider credentials as HTTP headers from trusted Rust code:

```rust
use agentic_harness::HttpSessionEnv;

let env = HttpSessionEnv::new("https://sandbox.example/session", "/workspace")
    .header("Authorization", format!("Bearer {}", token));
```

Do not put secrets in prompts or workspace context.

## Server Checklist

A compatible sandbox service should:

1. Authenticate every request before reading `op`.
2. Enforce the configured workspace root for all path operations.
3. Apply command timeouts.
4. Return non-2xx responses with a JSON `message` or `error.message`.
5. Avoid logging bearer tokens, payload secrets, or file contents by default.
