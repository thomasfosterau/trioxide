// Array pipelines over 20 000 records: filter/map/reduce chains, forEach, sort and slice.
import { makeOrders } from "./orders.js";
import { bigSpenders, quantityHistogram, revenue, revenueByCategory, stats } from "./report.js";

const ORDERS = makeOrders(20_000, 7);

export const run = (): number => {
  const total = revenue(ORDERS);
  const byCategory = revenueByCategory(ORDERS);
  const spenders = bigSpenders(ORDERS, 2500);
  const histogram = quantityHistogram(ORDERS);
  const summary = stats(ORDERS.map((o) => o.price));
  return (
    Math.round(total) +
    byCategory.reduce((a, b) => a + Math.round(b), 0) +
    spenders.reduce((a, b) => a * 31 + b, 0) % 1000003 +
    histogram.reduce((a, b) => a * 7 + b, 0) +
    Math.round(summary.mean * 1000 + summary.deviation * 1000 + summary.min + summary.max)
  );
};

export function main(): void {
  console.log(run());
}
