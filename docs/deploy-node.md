# Deploy on Node Hosts

Use this when a platform expects a Node start command but you want the agent
runtime to stay native Rust.

This target does not transpile the agent to JavaScript. It packages the native
agent binary plus a small `server.mjs` launcher.

Build the package:

```bash
# Equivalent long form: agentic-harness build --target node --workspace .
agentic-harness build --workspace . --target node --output ./build
```

The output directory contains:

- `agentic-harness-agent`: the compiled native Rust agent server.
- `manifest.json`: agent route metadata.
- `server.mjs`: Node launcher for the native server.
- `package.json`: private host package with `scripts.start`.

Run locally:

```bash
cd build/dist
PORT=3583 node server.mjs
```

Deploy by uploading `build/dist` to a Node host and setting the start command:

```bash
node server.mjs
```

The launcher reads `HOST` and `PORT`, defaults to `0.0.0.0:3583`, then starts
the native binary with `--agentic-harness-serve`.

## Verify

```bash
curl "http://127.0.0.1:${PORT:-3583}/health"
curl "http://127.0.0.1:${PORT:-3583}/agents"
```

If the host cannot execute native binaries, use a different target. The Node
package still requires the compiled Rust executable.
