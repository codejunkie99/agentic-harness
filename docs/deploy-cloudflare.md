# Deploy on Cloudflare Workers

Cloudflare output is for webhook/control agents and Worker-compatible adapters.
Coding work should still run in a local checkout, CI runner, or remote sandbox.

The generated Worker files are a boundary, not a complete runtime. A production
deployment must link an adapter that can actually invoke the Rust/WASM app.

Build Worker artifacts:

```bash
agentic-harness build --workspace . --target cloudflare --output ./build
```

If you already have an adapter, include it during the build:

```bash
agentic-harness build --workspace . --target cloudflare --output ./build \
  --worker-app ./worker-app.js \
  --worker-wasm ./agentic_harness_app.wasm
```

The output directory contains:

- `_entry.js`: Worker entrypoint and Durable Object router.
- `wrangler.jsonc`: bindings, migrations, main module, and compatibility date.
- `cloudflare-manifest.json`: generated agent routing metadata.
- `agentic_harness_worker.js`: Worker runtime shim.
- `agentic_harness_app.js`: app adapter boundary.
- `agentic_harness_app.d.ts`: adapter contract.

Deploy after linking a real Worker adapter:

```bash
npx wrangler deploy --config build/dist/wrangler.jsonc
```

The generated `agentic_harness_app.js` is a stub unless you pass `--worker-app`,
`--worker-wasm`, or `--worker-wasm-crate`. A production Worker must replace that
file with an adapter that exports `invokeAgenticHarnessAgent`.

Verify the generated metadata before deploying:

```bash
agentic-harness manifest --workspace . --cloudflare
```
