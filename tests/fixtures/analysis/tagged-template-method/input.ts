// A tagged template whose tag is a member expression is a method call: `p.f`x`` runs `f` with
// `this` = `p`, like `p.f(strings)`.
const p = {
  name: "p",
  f(this: { name: string } | undefined, strings: TemplateStringsArray, ...values: number[]): string {
    const kind = this === undefined ? "undef" : typeof this;
    return `${kind}:${this?.name}:${strings.join("|")}:${values.join(",")}`;
  },
};

function free(this: unknown, strings: TemplateStringsArray): string {
  return `${this === undefined ? "undef" : "bound"}:${strings[0]}`;
}

export function main(): void {
  console.log(p.f`x`);
  console.log(p.f`a${1}b${2}c`);
  console.log(free`y`);
}
