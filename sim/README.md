# MetroForge sim (`sim/`)

The TypeScript simulation that powers MetroForge: the deterministic sim core
(`src/core/`), the host loop and UI-state builders (`src/host/`), the scenario
and campaign content (`src/content/`), the baked city data (`src/data/cities/`),
and the Bun WebSocket **sidecar** (`sidecar/`) the native client spawns to drive
that sim without a browser. One sim implementation serves both the web build and
the desktop client, so the two cannot silently drift apart.

At runtime the sidecar is compiled with `bun build --compile` into a single-file
binary that ships next to the `metroforge` client and is spawned by `mf-net`.

```sh
cd sim
bun install                 # install dev deps (bun types, vitest, ajv, shapefile)
bun test                    # vitest sim suite (determinism, economy, scenarios, ...)
bun run sidecar/index.ts    # run the sidecar interpreted (--port 0 by default)
bun run smoke               # sidecar determinism smoke test (bun run sidecar/smoke-test.ts)
bun run compile:linux       # bun build --compile -> ../dist-sidecar/metroforge-sidecar
bun run build-cities        # regenerate src/data/cities/* from OSM (pipeline)
bun run validate-scenarios  # validate content/ scenario JSON against schema
```

See `sidecar/README.md` for the wire protocol, compile matrix, and the
determinism gate details.
