// Pass 12 (closure hoisting): closures that capture nothing, created on every iteration.
const OFFSET = 3;
const rows = [];
for (let i = 0; i < 64; i++) {
	rows.push([
		i,
		i + 1,
		i + 2,
		i + 3
	]);
}
export function bench() {
	let total = 0;
	for (let i = 0; i < rows.length; i++) {
		const weight = (v) => v * 2 + OFFSET;
		const doubled = rows[i].map((v) => v * 2);
		total += weight(i) + doubled[3];
	}
	return total;
}
