// Array pipelines (PLAN §4 pass 6): chains of `map`/`filter`/`reduce` and searches over arrays
// the program builds.

const readings: number[] = [];
for (let i = 0; i < 20_000; i++) readings.push(((i * 7919) % 1000) / 10);

interface Order {
  id: number;
  price: number;
  quantity: number;
}

const orders: Order[] = [];
for (let i = 0; i < 5_000; i++) {
  orders.push({ id: i, price: (i % 97) + 0.5, quantity: (i % 7) + 1 });
}

function summary(): number {
  const total = readings
    .map((x) => x * 1.8 + 32)
    .filter((f) => f > 50)
    .reduce((sum, f) => sum + f, 0);
  const anyHot = readings.some((x) => x > 99.5);
  const firstCold = readings.findIndex((x) => x < 0.5);
  return total + (anyHot ? 1 : 0) + firstCold;
}

function revenue(): number {
  const big = orders.filter((order) => order.quantity > 3).map((order) => order.price * order.quantity);
  let sum = 0;
  for (let i = 0; i < big.length; i++) sum += big[i];
  return sum;
}

export function bench(): number {
  return summary() + revenue();
}

export function main(): void {
  console.log(bench());
}
