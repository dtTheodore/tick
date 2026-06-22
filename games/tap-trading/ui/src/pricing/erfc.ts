/**
 * Complementary error function. Abramowitz & Stegun 7.1.26.
 * Max abs error: 1.5e-7 (vs libm erfc). Sufficient for our 1e-6 parity tolerance.
 */
export function erfc(x: number): number {
  const z = Math.abs(x);
  const t = 1 / (1 + 0.5 * z);
  const ans = t * Math.exp(
    -z * z - 1.26551223 +
    t * (1.00002368 +
    t * (0.37409196 +
    t * (0.09678418 +
    t * (-0.18628806 +
    t * (0.27886807 +
    t * (-1.13520398 +
    t * (1.48851587 +
    t * (-0.82215223 +
    t * 0.17087277))))))))
  );
  return x >= 0 ? ans : 2 - ans;
}
