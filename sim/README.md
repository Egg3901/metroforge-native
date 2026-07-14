# MetroForge sim (`sim/`)

The TypeScript simulation that powers MetroForge: the deterministic sim core
(`src/core/`), the host loop and UI-state builders (`src/host/`), the scenario
and campaign content (`src/content/`), plus baked city data
(`src/data/cities/`). This package is now reference/tooling content: runtime
simulation in the desktop client is the in-process Rust sim.

```sh
cd sim
bun install                 # install dev deps (bun types, vitest, ajv, shapefile)
bun test                    # vitest sim suite (determinism, economy, scenarios, ...)
bun run build-cities        # regenerate src/data/cities/* from OSM (pipeline)
bun run validate-scenarios  # validate content/ scenario JSON against schema
```
