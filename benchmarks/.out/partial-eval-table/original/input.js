// Pass 11 (partial evaluation): a lookup table built at module evaluation. The benchmark
// measures evaluating the module (`init`), where the table is now a literal.
function primes(count) {
	const found = [];
	for (let n = 2; found.length < count; n++) {
		let prime = true;
		for (let d = 2; d * d <= n; d++) {
			if (n % d === 0) {
				prime = false;
				break;
			}
		}
		if (prime) found.push(n);
	}
	return found;
}
function gamma(levels) {
	const table = [];
	for (let i = 0; i < levels; i++) {
		let v = i / (levels - 1);
		// Newton steps for the cube root, exact arithmetic only.
		let r = v;
		for (let k = 0; k < 20; k++) {
			if (r > 0) r = (2 * r + v / (r * r)) / 3;
		}
		table.push(Math.round(r * 255));
	}
	return table;
}
/** The harness times evaluating this module. */
export const measure = "init";
const PRIMES = primes(64);
const GAMMA = gamma(64);
export function bench() {
	return PRIMES[63] + GAMMA[32];
}
