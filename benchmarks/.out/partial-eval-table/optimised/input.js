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
const PRIMES = [
	2,
	3,
	5,
	7,
	11,
	13,
	17,
	19,
	23,
	29,
	31,
	37,
	41,
	43,
	47,
	53,
	59,
	61,
	67,
	71,
	73,
	79,
	83,
	89,
	97,
	101,
	103,
	107,
	109,
	113,
	127,
	131,
	137,
	139,
	149,
	151,
	157,
	163,
	167,
	173,
	179,
	181,
	191,
	193,
	197,
	199,
	211,
	223,
	227,
	229,
	233,
	239,
	241,
	251,
	257,
	263,
	269,
	271,
	277,
	281,
	283,
	293,
	307,
	311
];
const GAMMA = [
	0,
	64,
	81,
	92,
	102,
	110,
	116,
	123,
	128,
	133,
	138,
	143,
	147,
	151,
	154,
	158,
	161,
	165,
	168,
	171,
	174,
	177,
	180,
	182,
	185,
	187,
	190,
	192,
	195,
	197,
	199,
	201,
	203,
	206,
	208,
	210,
	212,
	214,
	215,
	217,
	219,
	221,
	223,
	225,
	226,
	228,
	230,
	231,
	233,
	235,
	236,
	238,
	239,
	241,
	242,
	244,
	245,
	247,
	248,
	249,
	251,
	252,
	254,
	255
];
export function bench() {
	return PRIMES[63] + GAMMA[32];
}
//# sourceMappingURL=input.js.map
