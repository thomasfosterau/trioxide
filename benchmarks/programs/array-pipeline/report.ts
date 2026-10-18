// Reporting over orders, written the way application code usually is: array pipelines with
// small callbacks.

import { CATEGORIES, type Order } from "./orders.js";

export function netTotal(order: Order): number {
  return order.quantity * order.price * (1 - order.discount);
}

export function revenue(orders: Order[]): number {
  return orders
    .filter((o) => !o.returned)
    .map((o) => netTotal(o))
    .reduce((sum, value) => sum + value, 0);
}

export function revenueByCategory(orders: Order[]): number[] {
  return CATEGORIES.map((_, category) =>
    orders
      .filter((o) => o.category === category && !o.returned)
      .map(netTotal)
      .reduce((sum, value) => sum + value, 0),
  );
}

export function bigSpenders(orders: Order[], threshold: number): number[] {
  const totals = new Array<number>(500).fill(0);
  orders.forEach((o) => {
    if (!o.returned) totals[o.customer] += netTotal(o);
  });
  return totals
    .map((total, customer) => ({ customer, total }))
    .filter((entry) => entry.total > threshold)
    .sort((a, b) => b.total - a.total || a.customer - b.customer)
    .slice(0, 10)
    .map((entry) => entry.customer);
}

export function quantityHistogram(orders: Order[]): number[] {
  return [1, 2, 3, 4, 5].map((quantity) => orders.filter((o) => o.quantity === quantity).length);
}

export function stats(values: number[]): { mean: number; deviation: number; min: number; max: number } {
  const mean = values.reduce((a, b) => a + b, 0) / values.length;
  const variance = values.map((v) => (v - mean) * (v - mean)).reduce((a, b) => a + b, 0) / values.length;
  return {
    mean,
    deviation: Math.sqrt(variance),
    min: values.reduce((a, b) => Math.min(a, b), Infinity),
    max: values.reduce((a, b) => Math.max(a, b), -Infinity),
  };
}
