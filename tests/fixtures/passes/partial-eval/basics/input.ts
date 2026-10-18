// Top-level calls with constant arguments are evaluated at compile time.
const SIZE = 8;

function squares(n: number): number[] {
  const table: number[] = [];
  for (let i = 0; i < n; i++) {
    table.push(i * i);
  }
  return table;
}

function crc8Table(): number[] {
  const table = new Array(16);
  for (let i = 0; i < 16; i++) {
    let c = i;
    for (let k = 0; k < 8; k++) {
      c = c & 1 ? (c >>> 1) ^ 0x8c : c >>> 1;
    }
    table[i] = c;
  }
  return table;
}

function label(name: string, version: number): { name: string; version: number; tag: string } {
  return { name, version, tag: `${name}@${version}` };
}

function numbers(): number[] {
  return [-0, 0 / 0, 1 / 0, 5e-324, 2 ** 53, 0.1 + 0.2, Math.round(-0.5), Math.max(-0, 0)];
}

const SQUARES = squares(SIZE);
const CRC = crc8Table();
const LABEL = label("trioxide", 2);
const NUMBERS = numbers();

export function main(): void {
  console.log(SQUARES.join(","), CRC.join(","), LABEL.tag, LABEL.version);
  console.log(NUMBERS.map((x) => Object.is(x, -0) ? "-0" : String(x)).join(","));
}
