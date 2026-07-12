/**
 * Weather (v0.7) tests: seeded-Markov determinism, climate-table sanity
 * (each monthly row is a probability distribution), effect bounds, and a
 * documented weather-sequence snapshot for a fixed seed.
 */
import { describe, expect, it } from 'vitest';
import {
  WEATHER_STATES,
  climateTable,
  monthForDay,
  seasonOfMonth,
  weatherAt,
  weatherDayState,
  type WeatherState,
} from '../src/core/weather';
import {
  weatherBuildCostMult,
  weatherCarPenaltyMin,
  weatherDemandMult,
  weatherSpeedMult,
  weatherWalkMult,
} from '../src/core/weatherEffects';
import { TICKS_PER_DAY } from '../src/core/constants';

const CITY_KEYS = [
  'generic', 'nyc', 'la', 'seattle', 'chicago', 'boston', 'atlanta', 'sf', 'dc', 'philly', 'cleveland',
];

describe('climate tables', () => {
  it('every city has 12 monthly rows that sum to 1 over 6 states', () => {
    for (const key of CITY_KEYS) {
      const table = climateTable(key);
      expect(table).toHaveLength(12);
      for (let m = 0; m < 12; m++) {
        const row = table[m]!;
        expect(row).toHaveLength(WEATHER_STATES.length);
        for (const p of row) expect(p).toBeGreaterThanOrEqual(0);
        const sum = row.reduce((a, b) => a + b, 0);
        expect(sum).toBeCloseTo(1, 10);
      }
    }
  });

  it('unknown city falls back to the generic profile', () => {
    expect(climateTable('atlantis')).toEqual(climateTable('generic'));
    expect(climateTable(undefined)).toEqual(climateTable('generic'));
  });

  it('reads like the real cities: LA basically never snows, Seattle is wet, NYC snows in winter', () => {
    const snowIdx = WEATHER_STATES.indexOf('snow');
    const rainIdx = WEATHER_STATES.indexOf('rain');
    const overcastIdx = WEATHER_STATES.indexOf('overcast');
    const clearIdx = WEATHER_STATES.indexOf('clear');
    // January (month 0, winter)
    const la = climateTable('la')[0]!;
    const nyc = climateTable('nyc')[0]!;
    const sea = climateTable('seattle')[0]!;
    expect(la[snowIdx]).toBe(0); // LA winter: no snow
    expect(nyc[snowIdx]).toBeGreaterThan(0.2); // NYC winter: real snow chance
    // Seattle is wetter/cloudier than LA
    expect(sea[rainIdx] + sea[overcastIdx]).toBeGreaterThan(la[rainIdx] + la[overcastIdx]);
    // LA is sunnier than Seattle
    expect(la[clearIdx]).toBeGreaterThan(sea[clearIdx]);
  });
});

describe('calendar', () => {
  it('maps months to the right seasons', () => {
    expect(seasonOfMonth(0)).toBe('winter'); // Jan
    expect(seasonOfMonth(3)).toBe('spring'); // Apr
    expect(seasonOfMonth(6)).toBe('summer'); // Jul
    expect(seasonOfMonth(9)).toBe('autumn'); // Oct
    expect(seasonOfMonth(11)).toBe('winter'); // Dec
  });

  it('derives month from the sim date (TICKS_PER_DAY)', () => {
    expect(monthForDay(0)).toBe(0);
    expect(monthForDay(364)).toBe(11);
    // a tick deep in "July" of year 2 lands in month 6
    const day = 365 + 190;
    expect(monthForDay(day)).toBe(6);
  });
});

describe('markov determinism', () => {
  it('same seed produces an identical weather sequence forever', () => {
    const table = climateTable('nyc');
    const seqA: WeatherState[] = [];
    const seqB: WeatherState[] = [];
    for (let day = 0; day < 730; day++) {
      seqA.push(weatherDayState(12345, day, table));
      seqB.push(weatherDayState(12345, day, table));
    }
    expect(seqA).toEqual(seqB);
  });

  it('is a pure function of (seed, tick): recomputing at any tick matches', () => {
    const table = climateTable('chicago');
    const seed = 999;
    // "resume" at tick T must equal a fresh compute at tick T
    for (const t of [0, 50, 1200, 5000, 100_000, 1_234_567]) {
      const a = weatherAt(seed, t, table);
      const b = weatherAt(seed, t, table);
      expect(a).toEqual(b);
    }
  });

  it('different seeds produce different weather sequences', () => {
    const table = climateTable('nyc');
    const seqA: WeatherState[] = [];
    const seqB: WeatherState[] = [];
    for (let day = 0; day < 365; day++) {
      seqA.push(weatherDayState(1, day, table));
      seqB.push(weatherDayState(2, day, table));
    }
    expect(seqA).not.toEqual(seqB);
  });

  it('weather clusters (persistence) rather than flip-flopping every day', () => {
    const table = climateTable('seattle');
    let same = 0;
    const N = 730;
    let prev = weatherDayState(42, 0, table);
    for (let day = 1; day < N; day++) {
      const cur = weatherDayState(42, day, table);
      if (cur === prev) same += 1;
      prev = cur;
    }
    // a memoryless iid chain would repeat far less; persistence lifts it well up
    expect(same / N).toBeGreaterThan(0.25);
  });
});

describe('weather-sequence snapshot (seed 12345, NYC, first 20 days)', () => {
  it('matches the documented golden sequence', () => {
    const table = climateTable('nyc');
    const seq: WeatherState[] = [];
    for (let day = 0; day < 20; day++) seq.push(weatherDayState(12345, day, table));
    // Golden snapshot — a change here means the chain math moved. Regenerate
    // intentionally if the model is retuned.
    expect(seq).toMatchSnapshot();
  });
});

describe('effect bounds', () => {
  const intensities = [0, 0.25, 0.5, 0.75, 1];
  it('demand multiplier stays in a sane band and eases to 1 at zero intensity', () => {
    for (const state of WEATHER_STATES) {
      for (const intensity of intensities) {
        const m = weatherDemandMult({ state, intensity, season: 'winter', month: 0 });
        expect(m).toBeGreaterThan(0.3);
        expect(m).toBeLessThanOrEqual(1.0001);
      }
      expect(weatherDemandMult({ state, intensity: 0, season: 'winter', month: 0 })).toBeCloseTo(1, 6);
    }
  });

  it('walk multiplier is in (0,1] and worst for snow/storm', () => {
    const rain = weatherWalkMult({ state: 'rain', intensity: 1, season: 'autumn', month: 9 });
    const snow = weatherWalkMult({ state: 'snow', intensity: 1, season: 'winter', month: 0 });
    expect(rain).toBeGreaterThan(0);
    expect(rain).toBeLessThanOrEqual(1);
    expect(snow).toBeLessThan(rain);
    // rain trims the walk catchment ~15%
    expect(rain).toBeCloseTo(0.85, 6);
  });

  it('car penalty is nonnegative and largest for storms', () => {
    const rain = weatherCarPenaltyMin({ state: 'rain', intensity: 1, season: 'spring', month: 3 });
    const storm = weatherCarPenaltyMin({ state: 'storm', intensity: 1, season: 'summer', month: 6 });
    expect(rain).toBeGreaterThanOrEqual(0);
    expect(storm).toBeGreaterThan(rain);
    expect(weatherCarPenaltyMin({ state: 'storm', intensity: 0, season: 'summer', month: 6 })).toBe(0);
  });

  it('surface speed penalty respects grade separation (underground immune)', () => {
    const snow = { state: 'snow' as const, intensity: 1, season: 'winter' as const, month: 0 };
    const surface = weatherSpeedMult(snow, 'tram', 1);
    const underground = weatherSpeedMult(snow, 'metro', 0);
    expect(surface).toBeLessThan(1); // surface line slows in snow
    expect(underground).toBeCloseTo(1, 6); // fully underground line is untouched
    // ~-25% at full-strength snow on a fully exposed line
    expect(surface).toBeCloseTo(0.75, 6);
  });

  it('blizzard nearly stops the surface network but not the tunnel', () => {
    const blizzard = { state: 'storm' as const, intensity: 1, season: 'winter' as const, month: 0, event: 'blizzard' as const };
    const surface = weatherSpeedMult(blizzard, 'tram', 1);
    const underground = weatherSpeedMult(blizzard, 'metro', 0);
    expect(surface).toBeLessThan(0.3);
    expect(underground).toBeCloseTo(1, 6);
  });

  it('heat wave restricts rail speed regardless of grade', () => {
    const heat = { state: 'clear' as const, intensity: 0.8, season: 'summer' as const, month: 6, event: 'heatwave' as const };
    const railUnderground = weatherSpeedMult(heat, 'metro', 0);
    const bus = weatherSpeedMult(heat, 'bus', 1);
    expect(railUnderground).toBeCloseTo(0.9, 6); // rail heat order applies underground too
    expect(bus).toBeCloseTo(1, 6); // buses ignore rail heat orders
  });

  it('build-cost surcharge is >= 1 and largest in a storm', () => {
    const clear = weatherBuildCostMult({ state: 'clear', intensity: 1, season: 'summer', month: 6 });
    const snow = weatherBuildCostMult({ state: 'snow', intensity: 1, season: 'winter', month: 0 });
    const storm = weatherBuildCostMult({ state: 'storm', intensity: 1, season: 'summer', month: 6 });
    expect(clear).toBe(1);
    expect(snow).toBeGreaterThan(1);
    expect(storm).toBeGreaterThan(snow);
  });

  it('no weather (undefined) is a total no-op across every effect', () => {
    expect(weatherDemandMult(undefined)).toBe(1);
    expect(weatherWalkMult(undefined)).toBe(1);
    expect(weatherCarPenaltyMin(undefined)).toBe(0);
    expect(weatherSpeedMult(undefined, 'tram', 1)).toBe(1);
    expect(weatherBuildCostMult(undefined)).toBe(1);
  });
});
