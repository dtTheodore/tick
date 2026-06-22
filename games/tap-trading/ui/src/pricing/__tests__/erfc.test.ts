import { test, expect } from 'bun:test';
import { erfc } from '../erfc';

// Reference values: scipy.special.erfc, double precision.
const CASES: ReadonlyArray<readonly [number, number]> = [
  [0.0,  1.0],
  [0.5,  0.4795001221869535],
  [1.0,  0.15729920705028516],
  [1.5,  0.033894853524689274],
  [2.0,  0.004677734981047266],
  [3.0,  2.2090496998585437e-5],
  [-0.5, 1.5204998778130465],
  [-1.0, 1.8427007929497149],
  [-2.0, 1.9953222650189528],
];

for (const [x, expected] of CASES) {
  test(`erfc(${x}) ≈ ${expected}`, () => {
    const got = erfc(x);
    const tolerance = Math.abs(expected) > 1 ? 1e-6 : 1e-6;
    expect(Math.abs(got - expected)).toBeLessThan(tolerance);
  });
}
