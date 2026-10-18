// Points that never leave the loop become locals.
export function main(): void {
  let total = 0;
  for (let i = 0; i < 10; i++) {
    const p = { x: i, y: i * 2 };
    p.x += 1;
    p.y++;
    total += p.x * p.y;
    const pair = [i, i + 1];
    total += pair[0] - pair[1];
  }
  console.log(total);
}
