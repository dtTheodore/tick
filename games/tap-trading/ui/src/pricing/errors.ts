/** Mirrors PricingError::InvalidSpot. Caller pauses taps. */
export class InvalidSpot extends Error {
  constructor(public value: number) { super(`invalid spot: ${value}`); }
}

/** Mirrors PricingError::InvalidSigma. Caller pauses taps. */
export class InvalidSigma extends Error {
  constructor(public value: number) { super(`invalid sigma: ${value}`); }
}
