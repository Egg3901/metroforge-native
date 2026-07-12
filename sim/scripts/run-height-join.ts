/**
 * CLI driver for scripts/height-join.ts under vite-node.
 *
 * vite-node sets process.argv[1] to its own bin path (node_modules/.bin/
 * vite-node), not the target script, so height-join.ts's main guard
 * (`import.meta.url === file://${process.argv[1]}`) never fires and the
 * importer silently exits 0 having joined nothing (mirrors the exact issue
 * scripts/run-build-cities.ts documents). This driver rewrites argv[1] to the
 * real script path so the guard fires as intended:
 *
 *   npx vite-node scripts/run-height-join.ts <city> [--source=ms|overture]
 */
import { fileURLToPath } from 'node:url';
import { join, dirname } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2).filter((a) => a !== '--');
process.argv = [process.argv[0]!, join(here, 'height-join.ts'), ...args];
await import('./height-join.ts');
