// Pass 8 (scalar replacement): `{x, y}` points that never leave the loop body.
export function bench() {
	let total = 0;
	for (let i = 0; i < 4096; i++) {
		const p = {
			x: i * .5,
			y: i * .25
		};
		const q = {
			x: p.y - 1,
			y: p.x + 1
		};
		p.x += q.x;
		p.y -= q.y;
		total += p.x * q.y - p.y * q.x;
	}
	return total;
}
