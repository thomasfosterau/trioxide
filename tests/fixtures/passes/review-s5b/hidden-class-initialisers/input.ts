// Review: a declared field must not overwrite what an earlier field initialiser set.

class P {
  declare late?: number;
  x = this.init();
  init(): number {
    this.late = 5;
    return 1;
  }
}

export function main(): void {
  const p = new P();
  console.log(String(p.x) + p.late);
}
