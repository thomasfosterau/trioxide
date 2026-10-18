// Loop-invariant code motion (PLAN §4 pass 9): array lengths and pure math in loops that write
// other arrays.
const samples = [];
for (let i = 0; i < 4096; i++) samples.push(Math.sin(i / 50));
function normalise(gain) {
	const out = [];
	for (let i = 0, samplesLength = samples.length, sqrtGain = Math.sqrt(gain), log = Math.log(gain + 1); i < samplesLength; i++) {
		out.push(samples[i] * sqrtGain + log);
	}
	let sum = 0;
	for (let i = 0, outLength = out.length; i < outLength; i++) sum += out[i];
	return sum;
}
function smooth() {
	const out = [];
	for (let i = 0, samplesLength = samples.length; i < samplesLength; i++) {
		const before = i > 0 ? samples[i - 1] : samples[0];
		const after = i + 1 < samplesLength ? samples[i + 1] : samples[i];
		out.push((before + samples[i] + after) / 3);
	}
	return out[out.length - 1];
}
export function bench() {
	return normalise(2) + normalise(3) + smooth();
}
export function main() {
	console.log(bench());
}
//# sourceMappingURL=input.js.map
