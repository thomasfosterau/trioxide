import { mandelbrot } from "./kernel.js";

export function run(): number {
  return mandelbrot(200, 150, 200);
}

export function main(): void {
  console.log(run());
}
