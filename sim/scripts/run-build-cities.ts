/**
 * CLI driver for scripts/build-cities.ts under vite-node.
 *
 * vite-node sets process.argv[1] to its own bin path (node_modules/.bin/
 * vite-node), not the target script, so build-cities.ts's main guard
 * (`import.meta.url === file://${process.argv[1]}`) never fires and the
 * importer silently exits 0 having built nothing. This driver rewrites
 * argv[1] to the real script path and normalizes the optional city-key
 * argument before importing, so the guard fires exactly as intended:
 *
 *   npx vite-node scripts/run-build-cities.ts          # all configured cities
 *   npx vite-node scripts/run-build-cities.ts nyc      # one city
 */
import { fileURLToPath } from 'node:url';
import { join, dirname } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
// argv after the driver script path, minus any `--` separator npm/vite-node
// may pass through; first remaining token (if any) is the city key.
const args = process.argv.slice(2).filter((a) => a !== '--');
process.argv = [process.argv[0]!, join(here, 'build-cities.ts'), ...args];
await import(new URL('./build-cities.ts', import.meta.url).href);
