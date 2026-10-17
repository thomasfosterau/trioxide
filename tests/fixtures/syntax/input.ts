interface User {
  name: string;
  email?: string;
  address?: { city?: string };
  tags: readonly string[];
}

const users: User[] = [
  { name: "ada", email: "ada@example.com", address: { city: "London" }, tags: ["math"] },
  { name: "grace", tags: [] },
];

function cityOf(user: User): string {
  return user.address?.city ?? "unknown";
}

function first<T>(items: readonly T[]): T {
  return items[0]!;
}

const DEFAULTS = { retries: 3, timeout: 1000 } as const;

function configure({ retries = DEFAULTS.retries, ...rest }: Partial<{ retries: number; timeout: number; name: string }>) {
  return { retries, ...DEFAULTS, ...rest };
}

let assigned: number | undefined;
assigned ||= 1;
assigned &&= assigned + 1;

label: for (const i of [1, 2, 3]) {
  for (const j of [1, 2, 3]) {
    if (i * j === 4) break label;
  }
}

export function main(): void {
  console.log(users.map(cityOf).join(","), first(users).name);
  console.log(users[1].email?.toUpperCase(), users[0].email!.length);
  console.log(JSON.stringify(configure({ timeout: 5, name: "x" })));
  const [head, ...tail] = users.flatMap((u) => [u.name, ...u.tags]);
  console.log(head, tail, assigned);
  const big: bigint = 2n ** 64n;
  console.log(big.toString(), typeof big, 1_000_000);
  console.log(`${"tagged"}`.padStart(8, "."), String.raw`a\nb`);
}
