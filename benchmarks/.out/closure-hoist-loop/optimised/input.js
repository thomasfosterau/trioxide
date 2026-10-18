const map_callback = (v) => v * 2;
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
		const doubled = rows[i].map(map_callback);
		total += i * 2 + 3 + doubled[3];
	}
	return total;
}
//# sourceMappingURL=input.js.map
