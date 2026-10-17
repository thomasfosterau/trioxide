// Random programs in the numeric subset (docs/PLAN.md §8), run through the differential
// legs: types stripped, trioxide JS (`--no-wasm`), and trioxide JS+WASM, where every
// generated function is forced into an island with `"use wasm"`. All three must print the
// same.
//
// Each program is a function over two numbers, a Float64Array, an Int32Array and one other
// typed array (any kind): local variables (and `x++` as a value, also inside the right-hand
// side of an assignment to `x`), arithmetic, bitwise, logical and `Math` expressions,
// ternaries, typed array and `number[]` reads and writes (in and out of bounds), bounded
// `for`/`while`/`do…while` loops with `break`/`continue` (labelled or not), and calls to a
// helper and a recursive helper. `main()` calls each on edge values and prints the results
// and the arrays after the call.
//
// RANDOM_SEED (default 20261017) and RANDOM_PROGRAMS (default 500) pick the programs; a run
// prints its seed, so a failure can be reproduced, and `RANDOM_PROGRAMS=5000` runs longer.
// Programs are packed 40 to a module, one island each (13 by default, so island ids like `k1`
// and `k10` share a prefix).

import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { before, test } from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../..');
const seed = Number(process.env.RANDOM_SEED ?? 20261017) >>> 0;
const count = Number(process.env.RANDOM_PROGRAMS ?? 500);
const PER_MODULE = 40;
const work = path.join(here, '.out', 'random', String(seed));
const cacheDir = path.join(here, '.out', '.cache');

let trioxide = process.env.TRIOXIDE_BIN;

before(() => {
  if (!trioxide) {
    execFileSync('cargo', ['build', '--quiet', '-p', 'trioxide_cli'], { cwd: repo, stdio: 'inherit' });
    trioxide = path.join(repo, 'target/debug/trioxide');
  }
});

/** mulberry32: small, seedable, deterministic everywhere. */
function rng(state) {
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const LITERALS = ['0', '1', '2', '3', '7', '-1', '0.5', '-2.5', '1.5', '31', '32', '255', '1e9', '2147483647', '4294967295', '-2147483648', '0.1', '1e300', '(-0)', 'NaN', 'Infinity'];
const BINARY = ['+', '-', '*', '/', '%', '&', '|', '^', '<<', '>>', '>>>'];
// `Math` functions the backend computes exactly as V8 does (the others keep code in JS).
const UNARY_MATH = ['abs', 'floor', 'ceil', 'trunc', 'sqrt', 'round', 'sign', 'fround', 'tan', 'asin', 'acos', 'atan', 'expm1', 'log2'];
const COMPARE = ['<', '<=', '>', '>='];
const TYPED = ['Uint8Array', 'Uint8ClampedArray', 'Int8Array', 'Int16Array', 'Uint16Array', 'Uint32Array', 'Float32Array'];
const COUNTERS = ['i', 'j', 'n', 'm'];

/**
 * Generates one program: an exported function `name` plus two helpers it may call, `<name>h`
 * and the recursive `<name>r`. `random` is the PRNG. Returns the code and the kind of the
 * third typed array parameter.
 */
function program(random, name) {
  const pick = (items) => items[Math.floor(random() * items.length)];
  const chance = (p) => random() < p;
  const kind = pick(TYPED);
  // A `number[]` local, written only at constant indices (the analysis doesn't converge on
  // `number[]` writes at loop indices yet).
  const zsLength = chance(0.4) ? pick([3, 4, 6]) : 0;
  let counter = 0;
  // Which helpers the function calls (each forced helper needs a caller to type it).
  const used = { h: false, r: false };
  // What expressions may use: helpers see only their parameters.
  let cx = {
    locals: ['a', 'b'],
    arrays: ['xs', 'ys', 'us', ...(zsLength ? ['zs'] : [])],
    calls: true,
  };
  const assignable = () => cx.locals.filter((local) => !COUNTERS.includes(local));

  const index = (depth) => {
    const base = pick(['i', 'j', '0', '1', '3', '7', 'n', 'm']);
    const usable = COUNTERS.includes(base) && !cx.locals.includes(base) ? '0' : base;
    const roll = random();
    if (roll < 0.25) return `${usable} + ${pick(['1', '-1', '2'])}`;
    if (roll < 0.35) return `(${expr(depth + 1)}) | 0`;
    if (roll < 0.42 && assignable().length) return `(${pick(assignable())} | 0)`;
    return usable;
  };

  const leaf = (depth) => {
    const roll = random();
    if (roll < 0.33) return pick(LITERALS);
    if (roll < 0.68 || !cx.arrays.length) {
      const targets = assignable();
      // `x++` as a value.
      if (targets.length && chance(0.06)) {
        const t = pick(targets);
        return chance(0.5) ? `(${t}${pick(['++', '--'])})` : `(${pick(['++', '--'])}${t})`;
      }
      return pick(cx.locals);
    }
    if (roll < 0.95) return `${pick(cx.arrays)}[${index(depth)}]`;
    return `${pick(cx.arrays)}.length`;
  };

  const expr = (depth = 0) => {
    if (depth > 3 || chance(0.25)) return leaf(depth);
    const roll = random();
    if (roll < 0.4) return `(${expr(depth + 1)} ${pick(BINARY)} ${expr(depth + 1)})`;
    if (roll < 0.48) return `${pick(['-', '+', '~'])}(${expr(depth + 1)})`;
    if (roll < 0.6) return `Math.${pick(UNARY_MATH)}(${expr(depth + 1)})`;
    if (roll < 0.66) return `Math.${pick(['min', 'max'])}(${expr(depth + 1)}, ${expr(depth + 1)}${chance(0.3) ? `, ${expr(depth + 1)}` : ''})`;
    if (roll < 0.69) return `Math.${pick(['imul', 'hypot'])}(${expr(depth + 1)}, ${expr(depth + 1)})`;
    if (roll < 0.7) return `Math.clz32(${expr(depth + 1)})`;
    if (roll < 0.72) return `(${expr(depth + 1)}) ** 2`;
    // `&&`/`||` on numbers.
    if (roll < 0.77) return `(${expr(depth + 1)} ${pick(['&&', '||'])} ${expr(depth + 1)})`;
    if (roll < 0.83 && cx.calls) {
      // Arguments are numbers (never `undefined`), as the helpers return them.
      if (chance(0.6)) {
        used.h = true;
        return `${name}h(+(${expr(depth + 1)}), +(${expr(depth + 1)}))`;
      }
      used.r = true;
      return `${name}r(${pick(['0', '1', '2', '5'])}, +(${expr(depth + 1)}))`;
    }
    // A number's truthiness, or a condition.
    const test = chance(0.2) ? expr(depth + 1) : condition(depth + 1);
    return `(${test} ? ${expr(depth + 1)} : ${expr(depth + 1)})`;
  };

  const condition = (depth) => {
    const roll = random();
    if (roll < 0.5) return `${expr(depth + 1)} ${pick(COMPARE)} ${expr(depth + 1)}`;
    if (roll < 0.62) return `(${expr(depth + 1)} | 0) ${pick(['===', '!=='])} (${expr(depth + 1)} | 0)`;
    // Truthiness of a number.
    if (roll < 0.72) return `${pick(['!', '!!'])}(${expr(depth + 1)})`;
    if (roll < 0.88) return `(${condition(depth + 1)}) ${pick(['&&', '||'])} (${condition(depth + 1)})`;
    return `!(${condition(depth + 1)})`;
  };

  /** Statements in a nested block: locals declared there go out of scope after it. */
  const block = (depth, budget, loops) => {
    const scope = cx.locals.length;
    const lines = statements(depth, budget, loops);
    cx.locals.length = scope;
    return lines;
  };

  /** A loop statement's label, if it gets one. */
  const label = (depth) => (depth < 2 && chance(0.3) ? `L${counter++}` : undefined);

  const loop = (depth, loops, counterName, head, tail) => {
    const name = label(depth);
    cx.locals.push(counterName);
    const body = block(depth + 1, 2, [...loops, name ?? '']);
    cx.locals.pop();
    return [...(name ? [`${name}:`] : []), ...head, ...body, ...tail];
  };

  const statements = (depth, budget, loops) => {
    const lines = [];
    for (let k = 0; k < budget; k++) {
      const roll = random();
      if (roll < 0.2) {
        const local = `v${counter++}`;
        lines.push(`let ${local} = ${expr()};`);
        cx.locals.push(local);
      } else if (roll < 0.36) {
        // Loop counters are never assigned, so every loop ends.
        const target = pick(assignable());
        // Sometimes the right-hand side also changes the target.
        const rhs = chance(0.25) ? `(${expr()} ${pick(BINARY)} ${target}${pick(['++', '--'])})` : expr();
        lines.push(`${target} ${pick(['=', '+=', '-=', '*=', '|=', '^='])} ${rhs};`);
      } else if (roll < 0.46) {
        lines.push(`${pick(['xs', 'ys', 'us'])}[${index(1)}] ${pick(['=', '+=', '-='])} ${expr()};`);
      } else if (roll < 0.5 && zsLength) {
        const at = Math.floor(random() * zsLength);
        lines.push(chance(0.5) ? `zs[${at}] = +(${expr()});` : `zs[${at}] ${pick(['+=', '-=', '*='])} ${expr()};`);
      } else if (roll < 0.55 && loops.length) {
        const target = pick(loops);
        lines.push(`if (${condition(0)}) ${pick(['break', 'continue'])}${target ? ` ${target}` : ''};`);
      } else if (roll < 0.63 && depth < 2) {
        const test = condition(0);
        const then = block(depth + 1, 2, loops);
        const otherwise = chance(0.5) ? ['} else {', ...block(depth + 1, 1, loops)] : [];
        lines.push(`if (${test}) {`, ...then, ...otherwise, '}');
      } else if (roll < 0.75 && depth < 2 && !cx.locals.includes('i') && !cx.locals.includes('j')) {
        const counterName = depth === 0 ? 'i' : 'j';
        const bound = chance(0.6) ? `${pick(cx.arrays)}.length` : pick(['3', '5']);
        lines.push(...loop(depth, loops, counterName, [`for (let ${counterName} = 0; ${counterName} < ${bound}; ${counterName}++) {`], ['}']));
      } else if (roll < 0.81 && depth < 2 && !cx.locals.includes('n')) {
        // The counter goes up first, so `continue` can't skip it.
        lines.push('{', 'let n = 0;', ...loop(depth, loops, 'n', [`while (n < ${pick(['2', '4'])}) {`, 'n++;'], ['}']), '}');
      } else if (roll < 0.86 && depth < 2 && !cx.locals.includes('m')) {
        lines.push('{', 'let m = 0;', ...loop(depth, loops, 'm', ['do {', 'm++;'], [`} while (m < ${pick(['2', '3'])});`]), '}');
      } else {
        const array = pick(cx.arrays);
        lines.push(array === 'zs' ? `zs[${Math.floor(random() * zsLength)}] = +(${expr()});` : `${array}[${index(1)}] = ${expr()};`);
      }
    }
    return lines;
  };

  const body = [
    ...(zsLength ? [`const zs: number[] = new Array(${zsLength}).fill(0);`] : []),
    ...statements(0, 3 + Math.floor(random() * 4), []),
  ];
  const result = `+(${expr()})`;

  // The helpers: arithmetic on their parameters only.
  const outer = cx;
  cx = { locals: ['x', 'y'], arrays: [], calls: false };
  const helper = expr(1);
  cx = { locals: ['k', 'x'], arrays: [], calls: false };
  const step = expr(1);
  cx = outer;

  const code = [
    ...(used.h ? [`function ${name}h(x: number, y: number): number {`, `  return +(${helper});`, '}', ''] : []),
    ...(used.r ? [`function ${name}r(k: number, x: number): number {`, `  return k <= 0 ? x : ${name}r(k - 1, +(${step}));`, '}', ''] : []),
    `export function ${name}(a: number, b: number, xs: Float64Array, ys: Int32Array, us: ${kind}): number {`,
    ...body.map((line) => `  ${line}`),
    `  return ${result};`,
    '}',
  ].join('\n');
  return { code, kind };
}

const INPUTS = [
  ['1', '2', '[1, 2, 3, 4]', '[5, -6, 7]', '[1, 250, 3]'],
  ['-0', 'NaN', '[]', '[2147483647]', '[]'],
  ['0.5', '-3', '[NaN, -0, Infinity]', '[0, -1, 2, 3, 4, 5]', '[0.5, -7, 300, 65537]'],
  ['1e10', '2147483648', '[0.1, 0.2]', '[]', '[1e10]'],
];

function sources() {
  const random = rng(seed);
  const modules = [];
  const kinds = new Map();
  for (let start = 0; start < count; start += PER_MODULE) {
    const names = [];
    const functions = [];
    for (let k = start; k < Math.min(count, start + PER_MODULE); k++) {
      const { code, kind } = program(random, `p${k}`);
      names.push(`p${k}`);
      functions.push(code);
      kinds.set(`p${k}`, kind);
    }
    modules.push({ file: `k${modules.length}.ts`, names, code: `"use wasm";\n\n${functions.join('\n\n')}\n` });
  }
  const imports = modules.map((m) => `import { ${m.names.join(', ')} } from "./${m.file.replace('.ts', '.js')}";`);
  const all = modules.flatMap((m) => m.names);
  // Direct calls, so the analysis sees every argument.
  const main = [
    ...imports,
    '',
    'const show = (x: number): string => (Object.is(x, -0) ? "-0" : String(x));',
    '',
    'function run(name: string, results: string[]): void {',
    '  console.log([name, ...results].join(" "));',
    '}',
    '',
    'export function main(): void {',
    ...all.map((name) => [
      '  {',
      '    const r: string[] = [];',
      ...INPUTS.map(([a, b, xs, ys, us]) =>
        `    { const xs = new Float64Array(${xs}); const ys = new Int32Array(${ys}); const us = new ${kinds.get(name)}(${us}); r.push(show(${name}(${a}, ${b}, xs, ys, us)), Array.from(xs, show).join(","), ys.join(","), Array.from(us, show).join(",")); }`),
      `    run("${name}", r);`,
      '  }',
    ].join('\n')),
    '}',
    '',
  ].join('\n');
  return { modules, main };
}

function execute(entry) {
  const script = `const m = await import(${JSON.stringify(pathToFileURL(entry).href)}); await m.main();`;
  return spawnSync(process.execPath, ['--input-type=module', '--eval', script], {
    encoding: 'utf8',
    cwd: path.dirname(entry),
    timeout: 120_000,
    maxBuffer: 256 * 1024 * 1024,
  });
}

function prepare(dir) {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  writeFileSync(path.join(dir, 'package.json'), '{ "type": "module" }\n');
}

test(`${count} random programs (seed ${seed}) agree in JS and WASM`, { timeout: 30 * 60_000 }, () => {
  const src = path.join(work, 'src');
  prepare(src);
  const { modules, main } = sources();
  for (const module of modules) writeFileSync(path.join(src, module.file), module.code);
  writeFileSync(path.join(src, 'input.ts'), main);
  // The generated `main` passes every array through an `Object.entries` loop, which the
  // analysis only follows through TypeScript's types.
  writeFileSync(path.join(src, 'trioxide.json'), JSON.stringify({ trustTypes: true, cacheDir }));
  writeFileSync(path.join(src, 'tsconfig.json'), JSON.stringify({ compilerOptions: { target: 'es2022', module: 'esnext', moduleResolution: 'bundler', strict: true, noEmit: true } }));

  const a = path.join(work, 'a');
  prepare(a);
  for (const file of [...modules.map((m) => m.file), 'input.ts']) {
    const code = execFileSync(trioxide, ['strip', '--root', src, path.join(src, file)], { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
    writeFileSync(path.join(a, file.replace('.ts', '.js')), code);
  }
  const stripped = execute(path.join(a, 'input.js'));
  assert.equal(stripped.status, 0, stripped.stderr);

  const b = path.join(work, 'b');
  prepare(b);
  execFileSync(trioxide, ['build', '--emit=js', '--no-wasm', '--root', src, '--out-dir', b, path.join(src, 'input.ts')]);
  const js = execute(path.join(b, 'input.js'));
  assert.equal(js.status, 0, js.stderr);
  assert.equal(js.stdout, stripped.stdout, 'trioxide JS differs from stripped JS');

  const c = path.join(work, 'c');
  prepare(c);
  const build = spawnSync(trioxide, ['build', '--emit=js', '--require-wasm', '--root', src, '--out-dir', c, path.join(src, 'input.ts')], { encoding: 'utf8' });
  assert.equal(build.status, 0, `seed ${seed}: build failed:\n${build.stderr}`);
  assert.doesNotMatch(build.stderr, /warning|error/, build.stderr);
  const wasm = execute(path.join(c, 'input.js'));
  assert.equal(wasm.status, 0, wasm.stderr);
  const expected = stripped.stdout.split('\n');
  const actual = wasm.stdout.split('\n');
  for (let line = 0; line < expected.length; line++) {
    assert.equal(actual[line], expected[line], `seed ${seed}: program ${expected[line]?.split(' ')[0]} differs in WASM (sources in ${src})`);
  }
  assert.equal(actual.length, expected.length);
});
