// Switch fallthrough, labelled breaks and continues.

function classify(n: number): string {
  let kind = "other";
  switch (n % 3) {
    case 0:
      kind = "fizz";
      break;
    case 1:
    case 2:
      kind = n > 10 ? "big" : "small";
      break;
    default:
      kind = "never";
  }
  return kind /*@proven string*/;
}

function search(grid: number[][], target: number): number {
  let found = -1;
  outer: for (let row = 0; row < grid.length; row++) {
    for (let column = 0; column < grid[row].length; column++) {
      if (grid[row][column] === target) {
        found = row * 10 + column;
        break outer;
      }
      if (grid[row][column] < 0) continue outer;
    }
  }
  return found /*@proven number int*/;
}

export function main(): void {
  console.log(classify(3), classify(13), search([[1, 2], [3, 4]], 3));
}
