// Dense n×n matrix kernels over row-major Float64Arrays. Exports call each other (power
// calls multiply and identity) and internal helpers (at, swapRows).

function at(n: number, row: number, col: number): number {
  return row * n + col;
}

function swapRows(a: Float64Array, n: number, r1: number, r2: number): void {
  for (let k = 0; k < n; k++) {
    const t = a[at(n, r1, k)];
    a[at(n, r1, k)] = a[at(n, r2, k)];
    a[at(n, r2, k)] = t;
  }
}

export function identity(n: number): Float64Array {
  const m = new Float64Array(n * n);
  for (let i = 0; i < n; i++) m[at(n, i, i)] = 1;
  return m;
}

export function multiply(a: Float64Array, b: Float64Array, n: number): Float64Array {
  const out = new Float64Array(n * n);
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < n; j++) {
      let s = 0;
      for (let k = 0; k < n; k++) s += a[at(n, i, k)] * b[at(n, k, j)];
      out[at(n, i, j)] = s;
    }
  }
  return out;
}

export function power(m: Float64Array, n: number, k: number): Float64Array {
  let result = identity(n);
  let base = m;
  while (k > 0) {
    if (k % 2 === 1) result = multiply(result, base, n);
    base = multiply(base, base, n);
    k = Math.floor(k / 2);
  }
  return result;
}

export function trace(m: Float64Array, n: number): number {
  let t = 0;
  for (let i = 0; i < n; i++) t += m[at(n, i, i)];
  return t;
}

export function determinant(m: Float64Array, n: number): number {
  const a = new Float64Array(m.length);
  for (let i = 0; i < m.length; i++) a[i] = m[i];
  let det = 1;
  for (let col = 0; col < n; col++) {
    let pivot = col;
    for (let row = col + 1; row < n; row++) {
      if (Math.abs(a[at(n, row, col)]) > Math.abs(a[at(n, pivot, col)])) pivot = row;
    }
    if (a[at(n, pivot, col)] === 0) return 0;
    if (pivot !== col) {
      swapRows(a, n, pivot, col);
      det = -det;
    }
    det *= a[at(n, col, col)];
    for (let row = col + 1; row < n; row++) {
      const f = a[at(n, row, col)] / a[at(n, col, col)];
      for (let k = col; k < n; k++) a[at(n, row, k)] -= f * a[at(n, col, k)];
    }
  }
  return det;
}

/** y += alpha * x */
export function axpy(y: Float64Array, x: Float64Array, alpha: number): void {
  for (let i = 0; i < y.length; i++) y[i] += alpha * x[i];
}
