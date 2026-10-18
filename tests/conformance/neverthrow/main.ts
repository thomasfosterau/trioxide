// Conformance driver (not part of neverthrow): a deterministic tour of the API whose output
// the harness compares across builds.
import { err, errAsync, fromThrowable, ok, okAsync, Result, ResultAsync, safeTry } from "./src/index.js";

function parse(text: string): Result<number, string> {
  const n = Number(text);
  return Number.isFinite(n) ? ok(n) : err(`not a number: ${text}`);
}

function half(n: number): Result<number, string> {
  return n % 2 === 0 ? ok(n / 2) : err(`odd: ${n}`);
}

const show = (r: Result<unknown, unknown>) =>
  r.match(
    (v) => `ok(${JSON.stringify(v)})`,
    (e) => `err(${JSON.stringify(e)})`,
  );

export async function main(): Promise<void> {
  for (const text of ["8", "6", "7", "x", "1e3"]) {
    const r = parse(text).andThen(half).map((n) => n * 10).mapErr((e) => e.toUpperCase());
    console.log(text, show(r), r.isOk(), r.unwrapOr(-1));
  }
  console.log("combine", show(Result.combine([ok(1), ok(2), ok(3)])));
  console.log("combine err", show(Result.combine([ok(1), err("a"), err("b")])));
  console.log("all errors", show(Result.combineWithAllErrors([ok(1), err("a"), err("b")])));
  const safeJson = fromThrowable(JSON.parse, (e) => (e as Error).name);
  console.log("json", show(safeJson('{"a":1}')), show(safeJson("{")));
  console.log("orElse", show(err<number, string>("bad").orElse((e) => ok(e.length))));
  console.log("andTee", show(ok(5).andTee((n) => console.log("tee", n)).map((n) => n + 1)));
  const gen = safeTry(function* () {
    const a = yield* parse("10");
    const b = yield* half(a);
    const c = yield* half(b);
    return ok(a + b + c);
  });
  console.log("safeTry", show(gen));
  const async1 = await okAsync(3)
    .map(async (n) => n * 2)
    .andThen((n) => (n > 5 ? okAsync(n) : errAsync("small")));
  console.log("async", show(async1));
  const async2 = await ResultAsync.fromPromise(Promise.reject(new Error("boom")), (e) => (e as Error).message);
  console.log("fromPromise", show(async2));
  const all = await ResultAsync.combine([okAsync(1), okAsync(2), errAsync("late")]);
  console.log("combine async", show(all));
}
