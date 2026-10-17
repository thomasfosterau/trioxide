namespace Geometry {
  export const EPSILON = 1e-9;

  export namespace Vec {
    export type T = [number, number];
    export function add(a: T, b: T): T {
      return [a[0] + b[0], a[1] + b[1]];
    }
    export function length(v: T): number {
      return Math.hypot(...v);
    }
  }

  export function nearlyEqual(a: number, b: number): boolean {
    return Math.abs(a - b) < EPSILON;
  }
}

namespace Geometry {
  export const ORIGIN: Vec.T = [0, 0];
}

function Counter(): number {
  return ++Counter.calls;
}
namespace Counter {
  export let calls = 0;
}

declare const DEBUG: boolean | undefined;

export function main(): void {
  const v = Geometry.Vec.add([3, 0], [0, 4]);
  console.log(v.join(","), Geometry.Vec.length(v), Geometry.nearlyEqual(0.1 + 0.2, 0.3));
  console.log(Geometry.ORIGIN, typeof DEBUG);
  Counter();
  Counter();
  console.log(Counter.calls);
}
