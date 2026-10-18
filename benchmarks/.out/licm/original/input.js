// Loop-invariant code motion (PLAN §4 pass 9): array lengths and pure math in loops that write
// other arrays.
const samples = [];
for (let i = 0; i < 4096; i++) samples.push(Math.sin(i / 50));
function normalise(gain) {
	const out = [];
	for (let i = 0; i < samples.length; i++) {
		out.push(samples[i] * Math.sqrt(gain) + Math.log(gain + 1));
	}
	let sum = 0;
	for (let i = 0; i < out.length; i++) sum += out[i];
	return sum;
}
function smooth() {
	const out = [];
	for (let i = 0; i < samples.length; i++) {
		const before = i > 0 ? samples[i - 1] : samples[i];
		const after = i + 1 < samples.length ? samples[i + 1] : samples[i];
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
