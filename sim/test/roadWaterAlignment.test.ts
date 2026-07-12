/**
 * Regression: road vertices must not land in water (land masses line up with
 * roads). Guards against the half-cell renderer shift fixed in
 * crates/mf-render/src/terrain.rs (`water_space` cell-centre origin) as well as
 * any data-side projection/bbox drift in the city bake that would push the
 * coarse water field off the road grid.
 *
 * The check rebuilds the 96² sim water field from each committed 640² OSM mask
 * (same 7×7 area sample as the generator) and samples it at every road vertex
 * using the renderer's FIXED cell-centre convention.
 */
import { readFileSync, readdirSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { checkCity, THRESHOLD } from '../scripts/check-road-water-alignment';

const DIR = 'src/data/cities';

describe('road ↔ water alignment', () => {
  const files = readdirSync(DIR).filter((f) => f.endsWith('.json'));
  for (const f of files) {
    const json = JSON.parse(readFileSync(`${DIR}/${f}`, 'utf8'));
    if (!json.roads || !json.waterMask) continue;
    it(`${json.key}: <=${(THRESHOLD * 100).toFixed(0)}% road vertices on water`, () => {
      const r = checkCity(json);
      expect(r.fixedRate).toBeLessThanOrEqual(THRESHOLD);
    });
  }
});
