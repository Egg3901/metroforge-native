/**
 * Place-name banks for procedural cities — generic American flavor.
 *
 * All picks MUST flow through an Rng so a seed reproduces the same city.
 * Use `assignPlaceNames` from the generator after districts/parks exist.
 *
 * Naming model:
 *   street  = <root> <suffix>   (Elm Street, Jefferson Boulevard)
 *   park     = <root> Park | <feature> (Riverside Park, The Commons)
 *   district = <root> | <root> <area>  (Fairview, Bishop Heights)
 *   city     = <prefix?><root><ending?> (Springfield, Fort Clarion, Lake Aurora)
 */
import type { Rng } from '../rng';

// ── Street roots (~120) — trees, presidents, nature, ordinals, virtues ───────
export const STREET_ROOTS: readonly string[] = [
  'Elm', 'Oak', 'Maple', 'Cedar', 'Pine', 'Birch', 'Willow', 'Walnut', 'Chestnut',
  'Spruce', 'Aspen', 'Poplar', 'Sycamore', 'Cypress', 'Hickory', 'Magnolia', 'Dogwood',
  'Washington', 'Jefferson', 'Lincoln', 'Madison', 'Monroe', 'Jackson', 'Adams',
  'Franklin', 'Hamilton', 'Roosevelt', 'Wilson', 'Grant', 'Cleveland', 'Harrison',
  'Kennedy', 'Garfield', 'Tyler', 'Polk', 'Pierce', 'Hayes', 'Taft',
  'River', 'Lake', 'Hill', 'Valley', 'Forest', 'Meadow', 'Brook', 'Spring',
  'Summit', 'Ridge', 'Grove', 'Glen', 'Prairie', 'Highland', 'Woodland', 'Fern',
  'Sunset', 'Sunrise', 'Bay', 'Harbor', 'Bridge', 'Mill', 'Canal', 'Dock',
  'First', 'Second', 'Third', 'Fourth', 'Fifth', 'Sixth', 'Seventh', 'Eighth',
  'Ninth', 'Tenth', 'Eleventh', 'Twelfth',
  'Church', 'School', 'Market', 'Union', 'Commerce', 'Industry', 'Depot', 'Station',
  'Center', 'Main', 'Broad', 'High', 'Front', 'Water', 'Park', 'College',
  'Liberty', 'Freedom', 'Independence', 'Victory', 'Progress', 'Prospect', 'Hope',
  'Franklin', 'Clark', 'Baker', 'Cooper', 'Carter', 'Bishop', 'Foster', 'Warren',
  'Sherman', 'Grand', 'Central', 'State', 'Federal', 'Capitol', 'Court', 'Vine',
  'Cherry', 'Peach', 'Laurel', 'Holly', 'Rose', 'Clover', 'Sage', 'Juniper',
  'Beacon', 'Lantern', 'Harvest', 'Orchard', 'Garden', 'Pasture', 'Windmill',
];

// ── Street suffixes — weighted toward common ones ────────────────────────────
export const STREET_SUFFIXES: readonly string[] = [
  'Street', 'Avenue', 'Boulevard', 'Road', 'Lane', 'Drive', 'Court', 'Place',
  'Way', 'Terrace', 'Parkway', 'Circle', 'Trail', 'Row', 'Alley', 'Crossing',
];
// index-aligned weights (Street/Avenue/Road most common)
const SUFFIX_WEIGHTS: readonly number[] = [
  9, 8, 4, 7, 5, 5, 3, 3, 2, 2, 2, 2, 1, 1, 1, 1,
];

// ── Park roots + standalone park names ───────────────────────────────────────
export const PARK_ROOTS: readonly string[] = [
  'Riverside', 'Lakeside', 'Hillcrest', 'Fairmount', 'Highland', 'Meadowbrook',
  'Cedar Grove', 'Oakwood', 'Willow Creek', 'Elmwood', 'Forest Glen', 'Sunset',
  'Liberty', 'Memorial', 'Veterans', 'Founders', 'Heritage', 'Pioneer',
  'Washington', 'Lincoln', 'Franklin', 'Jefferson', 'Roosevelt', 'Kennedy',
  'Garfield', 'Prospect', 'Overlook', 'Greenfield', 'Brookdale', 'Fernwood',
];
export const PARK_FEATURES: readonly string[] = [
  'The Commons', 'The Green', 'The Esplanade', 'City Gardens', 'Botanical Gardens',
  'Central Green', 'The Arboretum', 'Waterfront Park', 'Harbor Green',
  'The Promenade', 'Founders Square', 'Veterans Field', 'Riverwalk',
];

// ── Neighborhood / district roots + area words ───────────────────────────────
export const DISTRICT_ROOTS: readonly string[] = [
  'Fairview', 'Riverton', 'Oakdale', 'Ashford', 'Bexley', 'Clarendon', 'Danforth',
  'Eastgate', 'Westbrook', 'Northfield', 'Southport', 'Brookhaven', 'Cedarhurst',
  'Glenwood', 'Kingsley', 'Lakemont', 'Millbrook', 'Norwood', 'Parkside',
  'Ravenswood', 'Sherwood', 'Thornton', 'Vernon', 'Whitfield', 'Ashbury',
  'Bishop', 'Carver', 'Dover', 'Elmhurst', 'Foxridge', 'Granby', 'Hartwell',
  'Ironside', 'Kensington', 'Larkspur', 'Montrose', 'Oldtown', 'Pinecrest',
  'Quarry', 'Rosedale', 'Stonegate', 'Tanner', 'Underhill', 'Vale', 'Weston',
];
export const DISTRICT_AREAS: readonly string[] = [
  'Heights', 'Hills', 'Park', 'Village', 'Gardens', 'Landing', 'Square', 'Point',
  'Crossing', 'Junction', 'Flats', 'Quarter', 'District', 'Commons', 'Row',
];

// ── City name parts ──────────────────────────────────────────────────────────
export const CITY_PREFIXES: readonly string[] = [
  'Fort', 'Lake', 'New', 'Port', 'Mount', 'Saint', 'North', 'South', 'East', 'West',
];
export const CITY_ROOTS: readonly string[] = [
  'Spring', 'Clarion', 'Aurora', 'Bethel', 'Camden', 'Dayton', 'Elgin', 'Fenwick',
  'Gables', 'Haven', 'Ithaca', 'Jasper', 'Kingston', 'Laurel', 'Marion', 'Newton',
  'Orion', 'Preston', 'Quincy', 'Raleigh', 'Salem', 'Trenton', 'Auburn', 'Verona',
  'Warren', 'Yardley', 'Ashland', 'Bristol', 'Concord', 'Denton', 'Easton',
];
export const CITY_ENDINGS: readonly string[] = [
  'field', 'ton', 'ville', 'burg', 'ford', 'dale', 'wood', 'port', 'boro', 'haven',
];

/** One street name, e.g. "Jefferson Boulevard". */
export function streetName(rng: Rng): string {
  const root = rng.pick(STREET_ROOTS);
  const suffix = STREET_SUFFIXES[rng.weighted(SUFFIX_WEIGHTS)] ?? 'Street';
  return `${root} ${suffix}`;
}

/** One park name, e.g. "Riverside Park" or "The Commons". */
export function parkName(rng: Rng): string {
  if (rng.chance(0.28)) return rng.pick(PARK_FEATURES);
  return `${rng.pick(PARK_ROOTS)} Park`;
}

/** One neighborhood name, e.g. "Fairview" or "Bishop Heights". */
export function districtName(rng: Rng): string {
  const root = rng.pick(DISTRICT_ROOTS);
  if (rng.chance(0.45)) return `${root} ${rng.pick(DISTRICT_AREAS)}`;
  return root;
}

/** One city name, e.g. "Springfield", "Fort Clarion", "Lake Aurora". */
export function cityName(rng: Rng): string {
  const root = rng.pick(CITY_ROOTS);
  if (rng.chance(0.3)) return `${rng.pick(CITY_PREFIXES)} ${root}`;
  return `${root}${rng.pick(CITY_ENDINGS)}`;
}

/**
 * Deterministically draw N unique names from a generator fn. Falls back to
 * numbered suffixes if the bank is exhausted, so callers can't deadlock.
 */
export function uniqueNames(rng: Rng, n: number, gen: (r: Rng) => string): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  let guard = 0;
  while (out.length < n && guard < n * 20) {
    guard++;
    const name = gen(rng);
    if (seen.has(name)) continue;
    seen.add(name);
    out.push(name);
  }
  let dup = 2;
  while (out.length < n) {
    out.push(`${gen(rng)} ${dup++}`);
  }
  return out;
}
