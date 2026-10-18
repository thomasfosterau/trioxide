// `for…of` over arrays (PLAN §4 pass 7) in hot loops, and array spreads.
const grid = [];
for (let y = 0; y < 200; y++) {
	const row = [];
	for (let x = 0; x < 200; x++) row.push(x * y % 17);
	grid.push(row);
}
const weights = [];
for (let i = 0; i < 64; i++) weights.push(i % 5);
function total() {
	let sum = 0;
	for (let i = 0, length = grid.length; i < length; i++) {
		const row = grid[i];
		for (let j = 0, length = row.length; j < length; j++) {
			const cell = row[j];
			sum += cell;
		}
	}
	return sum;
}
function weighted() {
	let sum = 0;
	for (let round = 0; round < 50; round++) {
		const copy = weights.slice();
		for (let i = 0, length = copy.length; i < length; i++) {
			const w = copy[i];
			sum += w * round;
		}
	}
	return sum;
}
export function bench() {
	return total() + weighted();
}
export function main() {
	console.log(bench());
}
//# sourceMappingURL=input.js.map
