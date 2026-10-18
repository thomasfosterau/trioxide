// Times one compiled program in a process of its own, so one variant's JIT state can't affect
// another's, and prints `{ ns, check }` as JSON: the median time of one call, and the
// stringified result of a call (the harness checks every variant computes the same).
//
// Usage: node [engine flags] worker.mjs <entry.js> <export> <run|init> <milliseconds> [glue.js…]
//
// `run` mode calls the module's `<export>()` after awaiting every glue module's `ready()`,
// warms up for a quarter of the time, then times batches of at least 10 ms for the rest and
// takes the median batch. `init` mode times evaluating the module afresh (a new URL each
// time, so it includes reading and parsing the file), then calling `<export>()` once.

import { pathToFileURL } from 'node:url';

const [entry, name, mode, time, ...glues] = process.argv.slice(2);
const budget = Number(time);
const url = pathToFileURL(entry).href;
const module = await import(url);
for (const glue of glues) await (await import(pathToFileURL(glue).href)).ready();
const call = module[name];
if (typeof call !== 'function') throw new Error(`${entry} exports no function \`${name}\``);
const check = JSON.stringify(await call());

let fresh = 0;
const once =
  mode === 'init'
    ? async () => {
        await (await import(`${url}?fresh=${fresh++}`))[name]();
      }
    : call;
const async = mode === 'init' || once.constructor.name === 'AsyncFunction';

const warmUntil = performance.now() + budget / 4;
let warm = 0;
while (performance.now() < warmUntil || warm < 3) {
  if (async) await once();
  else once();
  warm++;
}
let perBatch = 1;
for (;;) {
  const t0 = performance.now();
  for (let i = 0; i < perBatch; i++) {
    if (async) await once();
    else once();
  }
  if (performance.now() - t0 >= 10) break;
  perBatch *= 2;
}
const samples = [];
const until = performance.now() + (budget * 3) / 4;
while (samples.length < 5 || (performance.now() < until && samples.length < 200)) {
  const t0 = performance.now();
  for (let i = 0; i < perBatch; i++) {
    if (async) await once();
    else once();
  }
  samples.push(((performance.now() - t0) * 1e6) / perBatch);
}
samples.sort((a, b) => a - b);
process.stdout.write(JSON.stringify({ ns: samples[samples.length >> 1], check }));
