# HttpSessionEnv Protocol

`HttpSessionEnv` adapts a remote sandbox service to the `SessionEnv` trait. It
sends one JSON `POST` to the configured endpoint for each operation.

Native binaries can use `HttpSessionEnv::new(...)`, which uses the built-in
HTTP client. Non-native adapters can use `HttpSessionEnv::with_transport(...)`
and provide an `HttpSessionTransport` backed by platform HTTP, such as Worker
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

| op | Request fields | Successful response |
| --- | --- | --- |
| `exec` | `command`, `cwd`, `env`, optional `timeoutMs` | `ShellOutput` JSON: `stdout`, `stderr`, `exitCode` |
| `read` | `path` | JSON string |
| `write` | `path`, `content` for UTF-8 text or `contentBase64` for bytes | any JSON value |
| `stat` | `path` | `FileStat` JSON: `isFile`, `isDirectory`, `isSymbolicLink`, `size`, `modifiedUnixMs` |
| `readdir` | `path` | JSON array of entry names |
| `exists` | `path` | JSON boolean |
| `mkdir` | `path` | any JSON value |
| `rm` | `path`, `recursive` | any JSON value |

For text writes, Agentic Harness sends:

```json
{ "op": "write", "path": "notes.txt", "content": "alpha" }
```

For binary writes, it sends:

```json
{ "op": "write", "path": "bin/blob.dat", "contentBase64": "/wAB" }
```

## Errors

Any non-2xx response is converted into an `AgenticHarnessError::Handler`. If the
response body contains `{ "error": { "message": "..." } }` or `{ "message": "..." }`,
that message is included in the error.

## Auth

Pass provider credentials as HTTP headers from trusted Rust code:

```rust
use agentic_harness::HttpSessionEnv;

let env = HttpSessionEnv::new("https://sandbox.example/session", "/workspace")
    .header("Authorization", format!("Bearer {}", token));
```

Do not put secrets in prompts or workspace context.
