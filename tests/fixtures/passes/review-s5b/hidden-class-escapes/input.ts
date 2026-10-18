// Review: instances reach key observers through `arguments`, builtin array results and spread
// arguments.

class Q {
  x = 1;
  declare late?: number;
}

function viaArguments(_o: Q): string {
  // eslint-disable-next-line prefer-rest-params
  return `${Object.keys(arguments[0]).join(",")} ${"late" in arguments[0]}`;
}

function run(): string {
  const p = new Q();
  const a = viaArguments(p);
  const b = Object.keys([0].concat([p as never])[1] as object).join(",");
  const c = Object.keys(...([p] as [Q])).join(",");
  p.late = 2;
  return `${a} | ${b} | ${c} | ${p.late}`;
}

export function main(): void {
  console.log(run());
}
