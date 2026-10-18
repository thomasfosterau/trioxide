// Numeric kernels: a FIR filter, RMS and an integer checksum over typed arrays.

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

export function rms(values: Float64Array): number {
  let sum = 0;
  for (let i = 0; i < values.length; i++) sum += values[i] * values[i];
  return Math.sqrt(sum / values.length);
}

export function quantize(values: Float64Array, out: Int32Array): void {
  for (let i = 0; i < values.length; i++) out[i] = Math.round(values[i] * 32767) | 0;
}

export function checksum(samples: Int32Array): number {
  let h = 0x811c9dc5 | 0;
  for (let i = 0; i < samples.length; i++) {
    h = Math.imul(h ^ samples[i], 0x01000193);
    h = (h + (h << 13)) | 0;
    h ^= h >>> 7;
  }
  return h >>> 0;
}
