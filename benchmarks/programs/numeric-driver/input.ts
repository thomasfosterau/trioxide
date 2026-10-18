// A JS driver feeding numeric kernels frame by frame: synthesises a signal, filters each
// 2048-sample frame with a 63-tap low-pass FIR, measures it and checksums the quantised output.
import { checksum, fir, quantize, rms } from "./dsp.js";

const FRAME = 2048;
const TAPS = 63;

function lowPass(taps: number, cutoff: number): Float64Array {
  const h = new Float64Array(taps);
  const middle = (taps - 1) / 2;
  let sum = 0;
  for (let i = 0; i < taps; i++) {
    const x = i - middle;
    const sinc = x === 0 ? 2 * cutoff : Math.sin(2 * Math.PI * cutoff * x) / (Math.PI * x);
    const window = 0.54 - 0.46 * Math.cos((2 * Math.PI * i) / (taps - 1));
    h[i] = sinc * window;
    sum += h[i];
  }
  for (let i = 0; i < taps; i++) h[i] /= sum;
  return h;
}

const taps = lowPass(TAPS, 0.1);

export const run = (): number => {
  const input = new Float64Array(FRAME);
  const output = new Float64Array(FRAME);
  const quantized = new Int32Array(FRAME);
  let hash = 0;
  let energy = 0;
  for (let frame = 0; frame < 8; frame++) {
    for (let i = 0; i < FRAME; i++) {
      const t = frame * FRAME + i;
      input[i] = Math.sin(t * 0.01) + 0.3 * Math.sin(t * 0.37) + 0.1 * Math.sin(t * 1.3);
    }
    fir(input, taps, output);
    energy += rms(output);
    quantize(output, quantized);
    hash = (hash * 31 + checksum(quantized)) >>> 0;
  }
  return hash + Math.round(energy * 1e6);
};

export function main(): void {
  console.log(run());
}
