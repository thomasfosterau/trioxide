// Synthetic order records, generated deterministically.

export interface Order {
  id: number;
  customer: number;
  category: number;
  quantity: number;
  price: number;
  discount: number;
  returned: boolean;
}

export const CATEGORIES = ["books", "games", "garden", "kitchen", "music", "outdoor", "toys", "tools"];

function lcg(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 4294967296;
  };
}

export function makeOrders(count: number, seed: number): Order[] {
  const random = lcg(seed);
  const orders: Order[] = [];
  for (let id = 0; id < count; id++) {
    orders.push({
      id,
      customer: Math.floor(random() * 500),
      category: Math.floor(random() * CATEGORIES.length),
      quantity: 1 + Math.floor(random() * 5),
      price: Math.round(random() * 10000) / 100,
      discount: random() < 0.2 ? 0.1 : 0,
      returned: random() < 0.05,
    });
  }
  return orders;
}
