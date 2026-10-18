// Class instances made in the program, passed to a kernel.
export class Particle {
  x: number;
  vx: number;
  constructor(x: number, vx: number) {
    this.x = x;
    this.vx = vx;
  }
}

export function energy(p: Particle): number {
  return 0.5 * p.vx * p.vx + p.x;
}
