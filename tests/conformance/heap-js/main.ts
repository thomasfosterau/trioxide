// Conformance driver (not part of heap-js): a deterministic tour of the API whose output the
// harness compares across builds.
import Heap from "./src/Heap.js";

function lcg(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 4294967296;
  };
}

type Task = { name: string; priority: number };

export function main(): void {
  const random = lcg(42);
  const numbers: number[] = [];
  for (let i = 0; i < 200; i++) numbers.push(Math.floor(random() * 1000));

  const min = new Heap<number>();
  min.init(numbers.slice());
  const sorted: number[] = [];
  while (!min.isEmpty()) sorted.push(min.pop()!);
  console.log("min order", sorted.slice(0, 12).join(","), "…", sorted.slice(-5).join(","));

  const max = new Heap<number>(Heap.maxComparator);
  for (const n of numbers) max.push(n);
  console.log("max top 5", max.top(5).join(","), "bottom 3", max.bottom(3).join(","));
  console.log("size", max.size(), "peek", max.peek(), "contains 0", max.contains(0));
  max.setLimit(10);
  console.log("limited", max.size(), max.toArray().sort((a, b) => a - b).join(","));

  console.log("nlargest", Heap.nlargest(4, numbers).join(","));
  console.log("nsmallest", Heap.nsmallest(4, numbers).join(","));
  const arr = numbers.slice(0, 20);
  Heap.heapify(arr);
  Heap.heappush(arr, -5);
  console.log("heappop", Heap.heappop(arr), Heap.heappushpop(arr, 7), Heap.heapreplace(arr, 999));
  console.log("heaptop", Heap.heaptop(arr, 3).join(","), "heapbottom", Heap.heapbottom(arr, 3).join(","));

  const tasks = new Heap<Task>((a, b) => a.priority - b.priority || (a.name < b.name ? -1 : 1));
  const names = ["write", "test", "bench", "fix", "ship", "review", "plan"];
  names.forEach((name, i) => tasks.add({ name, priority: (i * 7) % 5 }));
  console.log("remove fix", tasks.remove({ name: "fix", priority: 0 }, (a, b) => a.name === b.name));
  const order: string[] = [];
  for (const task of tasks) order.push(`${task.name}:${task.priority}`);
  console.log("tasks", order.join(" "));
  console.log("print", Heap.print(new Heap<number>().clone()), String(Heap.print(Heap.heapify([5, 3, 8, 1]))).length);
}
