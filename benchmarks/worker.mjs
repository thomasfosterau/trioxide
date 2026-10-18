// Times one compiled version of one case with tinybench, in a process of its own (so one
// version's JIT state can't affect the other's), and prints its throughput as JSON.
// Usage: node [engine flags] worker.mjs <entry.js> <bench|init> <milliseconds>

import { pathToFileURL } from 'node:url';
import { Bench } from 'tinybench';

const [entry, mode, time] = process.argv.slice(2);
const url = pathToFileURL(entry).href;
const module = await import(url);
let fresh = 0;
const bench = new Bench({ time: Number(time) });
bench.add(
  'run',
  mode === 'init'
    ? async () => {
        (await import(`${url}?fresh=${fresh++}`)).bench();
      }
    : () => module.bench(),
);
await bench.run();
const { result } = bench.tasks[0];
console.log(JSON.stringify({ mean: result.throughput.mean, rme: result.latency.rme, check: module.bench() }));
