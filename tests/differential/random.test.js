// Random programs in the numeric subset (docs/PLAN.md §8), run through the differential
// legs: types stripped, trioxide JS (`--no-wasm`), and trioxide JS+WASM, where every
// generated function is forced into an island with `"use wasm"`. All three must print the
// same.
//
// Each program is a function over two numbers, a Float64Array and an Int32Array: local
// variables, arithmetic, bitwise and `Math` expressions, ternaries, typed array reads and
// writes (in and out of bounds), and bounded loops. `main()` calls each on edge values and
// prints the results and the arrays after the call.
//
// RANDOM_SEED (default 20261017) and RANDOM_PROGRAMS (default 500) pick the programs; a run
// prints its seed, so a failure can be reproduced, and `RANDOM_PROGRAMS=5000` runs longer.
// Programs are packed 50 to a module, one island each, so one cargo invocation builds them.

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
const PER_MODULE = 50;
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
const UNARY_MATH = ['abs', 'floor', 'ceil', 'trunc', 'sqrt', 'round', 'sign', 'fround'];
const COMPARE = ['<', '<=', '>', '>='];

/** Generates one program; `random` is the PRNG. */
function program(random, name) {
  const pick = (items) => items[Math.floor(random() * items.length)];
  const chance = (p) => random() < p;
  const locals = ['a', 'b'];
  let counter = 0;

  const index = (depth) => {
    const base = pick(['i', 'j', '0', '1', '3', '7', 'n']);
    const usable = ['i', 'j', 'n'].includes(base) && !locals.includes(base) ? '0' : base;
    return chance(0.3) ? `${usable} + ${pick(['1', '-1', '2'])}` : chance(0.15) ? `(${expr(depth + 1)}) | 0` : usable;
  };

  const expr = (depth = 0) => {
    if (depth > 3 || chance(0.25)) {
      const roll = random();
      if (roll < 0.35) return pick(LITERALS);
      if (roll < 0.75) return pick(locals);
      if (roll < 0.85) return `xs[${index(depth)}]`;
      if (roll < 0.95) return `ys[${index(depth)}]`;
      return pick(['xs.length', 'ys.length']);
    }
    const roll = random();
    if (roll < 0.45) return `(${expr(depth + 1)} ${pick(BINARY)} ${expr(depth + 1)})`;
    if (roll < 0.55) return `${pick(['-', '+', '~'])}(${expr(depth + 1)})`;
    if (roll < 0.7) return `Math.${pick(UNARY_MATH)}(${expr(depth + 1)})`;
    if (roll < 0.77) return `Math.${pick(['min', 'max'])}(${expr(depth + 1)}, ${expr(depth + 1)}${chance(0.3) ? `, ${expr(depth + 1)}` : ''})`;
    if (roll < 0.8) return `Math.${pick(['imul', 'hypot'])}(${expr(depth + 1)}, ${expr(depth + 1)})`;
    if (roll < 0.82) return `Math.clz32(${expr(depth + 1)})`;
    if (roll < 0.85) return `(${expr(depth + 1)}) ** 2`;
    return `(${condition(depth + 1)} ? ${expr(depth + 1)} : ${expr(depth + 1)})`;
  };

  const condition = (depth) => {
    const roll = random();
    if (roll < 0.6) return `${expr(depth + 1)} ${pick(COMPARE)} ${expr(depth + 1)}`;
    if (roll < 0.75) return `(${expr(depth + 1)} | 0) ${pick(['===', '!=='])} (${expr(depth + 1)} | 0)`;
    if (roll < 0.9) return `(${condition(depth + 1)}) ${pick(['&&', '||'])} (${condition(depth + 1)})`;
    return `!(${condition(depth + 1)})`;
  };

  /** Statements in a nested block: locals declared there go out of scope after it. */
  const block = (depth, budget) => {
    const scope = locals.length;
    const lines = statements(depth, budget);
    locals.length = scope;
    return lines;
  };

  const statements = (depth, budget) => {
    const lines = [];
    for (let k = 0; k < budget; k++) {
      const roll = random();
      if (roll < 0.25) {
        const local = `v${counter++}`;
        lines.push(`let ${local} = ${expr()};`);
        locals.push(local);
      } else if (roll < 0.45) {
        // Loop counters are never assigned, so every loop ends.
        const target = pick(locals.filter((name) => !['i', 'j', 'n'].includes(name)));
        lines.push(`${target} ${pick(['=', '+=', '-=', '*=', '|=', '^='])} ${expr()};`);
      } else if (roll < 0.6) {
        lines.push(`${pick(['xs', 'ys'])}[${index(1)}] ${pick(['=', '+=', '-='])} ${expr()};`);
      } else if (roll < 0.7 && depth < 2) {
        const test = condition(0);
        const then = block(depth + 1, 2);
        const otherwise = chance(0.5) ? ['} else {', ...block(depth + 1, 1)] : [];
        lines.push(`if (${test}) {`, ...then, ...otherwise, '}');
      } else if (roll < 0.82 && depth < 2 && !locals.includes('i') && !locals.includes('j')) {
        const name = depth === 0 ? 'i' : 'j';
        const bound = chance(0.6) ? pick(['xs.length', 'ys.length']) : pick(['3', '5']);
        locals.push(name);
        const body = block(depth + 1, 2);
        locals.pop();
        lines.push(`for (let ${name} = 0; ${name} < ${bound}; ${name}++) {`, ...body, '}');
      } else if (roll < 0.88 && depth < 2 && !locals.includes('n')) {
        locals.push('n');
        const body = block(depth + 1, 2);
        locals.pop();
        lines.push('{', 'let n = 0;', `while (n < ${pick(['2', '4'])}) {`, ...body, 'n++;', '}', '}');
      } else {
        lines.push(`${pick(['xs', 'ys'])}[${index(1)}] = ${expr()};`);
      }
    }
    return lines;
  };

  const body = statements(0, 3 + Math.floor(random() * 4));
  const result = `+(${expr()})`;
  return [
    `export function ${name}(a: number, b: number, xs: Float64Array, ys: Int32Array): number {`,
    ...body.map((line) => `  ${line}`),
    `  return ${result};`,
    '}',
  ].join('\n');
}

const INPUTS = [
  ['1', '2', '[1, 2, 3, 4]', '[5, -6, 7]'],
  ['-0', 'NaN', '[]', '[2147483647]'],
  ['0.5', '-3', '[NaN, -0, Infinity]', '[0, -1, 2, 3, 4, 5]'],
  ['1e10', '2147483648', '[0.1, 0.2]', '[]'],
];

function sources() {
  const random = rng(seed);
  const modules = [];
  for (let start = 0; start < count; start += PER_MODULE) {
    const names = [];
    const functions = [];
    for (let k = start; k < Math.min(count, start + PER_MODULE); k++) {
      names.push(`p${k}`);
      functions.push(program(random, `p${k}`));
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
      ...INPUTS.map(([a, b, xs, ys]) =>
        `    { const xs = new Float64Array(${xs}); const ys = new Int32Array(${ys}); r.push(show(${name}(${a}, ${b}, xs, ys)), Array.from(xs, show).join(","), ys.join(",")); }`),
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
