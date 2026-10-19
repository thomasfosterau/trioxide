// A 63-tap FIR filter over 2048-sample Float64Array frames (numeric-driver's `fir`): float
// multiply-adds with typed-array arguments marshalled at each call.
export function fir(input: Float64Array, taps: Float64Array, output: Float64Array): void {
  const n = input.length;
  const m = taps.length;
  for (let i = 0; i < n; i++) {
    let acc = 0;
    for (let k = 0; k < m; k++) {
      const j = i - k;
      if (j >= 0) acc += input[j] * taps[k];
    }
    output[i] = acc;
  }
}
