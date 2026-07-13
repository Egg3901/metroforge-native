/**
 * Deterministic PRNG — xoshiro128** with splitmix32 seeding.
 * All core randomness MUST flow through an Rng instance. The 4-word integer
 * state is part of the save format, so a native reimplementation can resume
 * a stream mid-game bit-for-bit.
 */

export type RngState = [number, number, number, number];

function splitmix32(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x9e3779b9) >>> 0;
    let z = s;
    z = Math.imul(z ^ (z >>> 16), 0x21f0aaad);
    z = Math.imul(z ^ (z >>> 15), 0x735a2d97);
    return (z ^ (z >>> 15)) >>> 0;
  };
}

function rotl(x: number, k: number): number {
  return ((x << k) | (x >>> (32 - k))) >>> 0;
}

export class Rng {
  private s: RngState;

  constructor(seed: number | RngState) {
    if (typeof seed === 'number') {
      const mix = splitmix32(seed >>> 0);
      this.s = [mix(), mix(), mix(), mix()];
      // avoid the all-zero degenerate state
      if ((this.s[0] | this.s[1] | this.s[2] | this.s[3]) === 0) this.s[0] = 1;
    } else {
      this.s = [seed[0] >>> 0, seed[1] >>> 0, seed[2] >>> 0, seed[3] >>> 0];
    }
  }

  state(): RngState {
    return [...this.s] as RngState;
  }

  /** Next uint32. */
  nextUint(): number {
    const [s0, s1, s2, s3] = this.s;
    const result = (Math.imul(rotl(Math.imul(s1, 5) >>> 0, 7), 9)) >>> 0;
    const t = (s1 << 9) >>> 0;
    let n2 = (s2 ^ s0) >>> 0;
    let n3 = (s3 ^ s1) >>> 0;
    const n1 = (s1 ^ n2) >>> 0;
    const n0 = (s0 ^ n3) >>> 0;
    n2 = (n2 ^ t) >>> 0;
    n3 = rotl(n3, 11);
    this.s = [n0, n1, n2, n3];
    return result;
  }

  /** Uniform float in [0, 1). 24-bit mantissa keeps it cheap and portable. */
  next(): number {
    return (this.nextUint() >>> 8) / 16777216;
  }

  /** Uniform float in [min, max). */
  range(min: number, max: number): number {
    return min + (max - min) * this.next();
  }

  /** Uniform integer in [min, max] inclusive. */
  int(min: number, max: number): number {
    return min + Math.floor(this.next() * (max - min + 1));
  }

  /** Bernoulli trial. */
  chance(p: number): boolean {
    return this.next() < p;
  }

  pick<T>(arr: readonly T[]): T {
    if (arr.length === 0) throw new Error('Rng.pick on empty array');
    return arr[this.int(0, arr.length - 1)] as T;
  }

  /** Weighted index pick; weights need not sum to 1. */
  weighted(weights: readonly number[]): number {
    let total = 0;
    for (const w of weights) total += w;
    if (total <= 0) return 0;
    let r = this.next() * total;
    for (let i = 0; i < weights.length; i++) {
      r -= weights[i] as number;
      if (r < 0) return i;
    }
    return weights.length - 1;
  }

  /** Derive an independent child stream (e.g. per-subsystem). */
  fork(tag: number): Rng {
    return new Rng((this.nextUint() ^ Math.imul(tag, 0x9e3779b9)) >>> 0);
  }

  /** Fisher–Yates in place. */
  shuffle<T>(arr: T[]): T[] {
    for (let i = arr.length - 1; i > 0; i--) {
      const j = this.int(0, i);
      const tmp = arr[i] as T;
      arr[i] = arr[j] as T;
      arr[j] = tmp;
    }
    return arr;
  }
}
