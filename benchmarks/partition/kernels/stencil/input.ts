import { conv } from "./kernel.js";

const src = new Float32Array(65536);
const dst = new Float32Array(65536);
for (let i = 0; i < 65536; i++) src[i] = (i * 7) % 13;

export const run = (): number => {
  conv(src, dst);
  return dst[100] + dst[60000];
};

export function main(): void {
  console.log(run());
}
