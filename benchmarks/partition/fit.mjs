// Fits the cost model's weights to results.json (written by run.mjs): least squares of the
// measured saving per run (JS time − WASM time) against the operation counts, calls and
// bytes of each kernel's island, each row scaled by the kernel's run time so short kernels
// count as much as long ones. Prints the fitted weights and, for each kernel, what they pick:
// WASM when the predicted saving per run exceeds the island's instantiation cost plus the
// threshold, as the cost model decides. "Lost" is the measured time per run a wrong pick
// gives up (ties lose nothing).
//
// Usage: node fit.mjs [--ridge 0.01] [--weights weights.json]
// With --weights, evaluates those weights (e.g. the defaults) instead of fitting;
// `instantiation` and `threshold` default to CostModel::default's.

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const results = JSON.parse(readFileSync(path.join(here, 'results.json'), 'utf8')).filter(
  (r) => r.wasm != null && r.model.features,
);
const ridgeAt = process.argv.indexOf('--ridge');
const ridge = ridgeAt > 0 ? Number(process.argv[ridgeAt + 1]) : 1e-3;

const NAMES = ['arithmetic', 'integer', 'conversion', 'wrapping', 'math', 'remainder', 'index', 'checkedIndex', 'iteration', 'crossing', 'aggregate', 'byte'];
const row = (f) => [
  f.arithmetic, f.integer, f.conversion ?? 0, f.wrapping ?? 0, f.math, f.remainder ?? 0, f.index, f.checkedIndex, f.iteration, -f.calls, -f.aggregates, -f.bytes,
];

// Normal equations of the scaled, ridge-regularised problem.
const n = NAMES.length;
const a = Array.from({ length: n }, () => new Array(n).fill(0));
const b = new Array(n).fill(0);
for (const r of results) {
  const scale = 1 / Math.max(r.js, r.wasm);
  const x = row(r.model.features).map((v) => v * scale);
  const y = (r.js - r.wasm) * scale;
  for (let i = 0; i < n; i++) {
    b[i] += x[i] * y;
    for (let j = 0; j < n; j++) a[i][j] += x[i] * x[j];
  }
}
const diag = a.map((r, i) => r[i]);
for (let i = 0; i < n; i++) a[i][i] += ridge * (diag[i] || 1);
// Gaussian elimination.
for (let i = 0; i < n; i++) {
  let pivot = i;
  for (let k = i + 1; k < n; k++) if (Math.abs(a[k][i]) > Math.abs(a[pivot][i])) pivot = k;
  [a[i], a[pivot]] = [a[pivot], a[i]];
  [b[i], b[pivot]] = [b[pivot], b[i]];
  for (let k = i + 1; k < n; k++) {
    const f = a[k][i] / a[i][i];
    for (let j = i; j < n; j++) a[k][j] -= f * a[i][j];
    b[k] -= f * b[i];
  }
}
const w = new Array(n).fill(0);
for (let i = n - 1; i >= 0; i--) {
  let sum = b[i];
  for (let j = i + 1; j < n; j++) sum -= a[i][j] * w[j];
  w[i] = sum / a[i][i];
}
const fitted = Object.fromEntries(NAMES.map((name, i) => [name, Number(w[i].toPrecision(3))]));
process.stdout.write(`fitted weights (ns): ${JSON.stringify(fitted)}\n\n`);
const weightsAt = process.argv.indexOf('--weights');
let instantiation = 300_000;
let threshold = 0;
if (weightsAt > 0) {
  const given = JSON.parse(readFileSync(process.argv[weightsAt + 1], 'utf8'));
  NAMES.forEach((name, i) => {
    w[i] = given[name] ?? 0;
  });
  instantiation = given.instantiation ?? instantiation;
  threshold = given.threshold ?? threshold;
  process.stdout.write(`evaluating ${JSON.stringify(given)}\n\n`);
}

let agree = 0;
let lost = 0;
const lines = ['| kernel | measured saving | predicted saving | faster | picks | lost per run |', '|---|---|---|---|---|---|'];
for (const r of results) {
  const predicted = row(r.model.features).reduce((sum, v, i) => sum + v * w[i], 0);
  // Within 5% either side is as fast.
  const tie = Math.max(r.js, r.wasm) / Math.min(r.js, r.wasm) < 1.05;
  const faster = tie ? 'tie' : r.wasm < r.js ? 'WASM' : 'JS';
  const picks = predicted - instantiation > threshold ? 'WASM' : 'JS';
  const right = tie || faster === picks;
  if (right) agree++;
  const loss = right ? 0 : Math.abs(r.js - r.wasm);
  lost += loss;
  lines.push(`| ${r.name} | ${Math.round(r.js - r.wasm)} | ${Math.round(predicted)} | ${faster} | ${picks} | ${loss ? `${(loss / 1e6).toFixed(2)} ms` : '—'} |`);
}
process.stdout.write(`${lines.join('\n')}\n\n${agree}/${results.length} agree; wrong picks lose ${(lost / 1e6).toFixed(2)} ms per run in all\n`);
