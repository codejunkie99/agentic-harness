# Deploy on Node Hosts

Use this when a platform expects a Node start command but you want the agent
runtime to stay native Rust.

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
