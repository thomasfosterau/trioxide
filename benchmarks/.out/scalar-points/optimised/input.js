// Pass 8 (scalar replacement): `{x, y}` points that never leave the loop body.
export function bench() {
	let total = 0;
	for (let i = 0; i < 4096; i++) {
		let p_x = i * .5, p_y = i * .25;
		const q_x = p_y - 1, q_y = p_x + 1;
		p_x += q_x;
		p_y -= q_y;
		total += p_x * q_y - p_y * q_x;
	}
	return total;
}
//# sourceMappingURL=input.js.map
