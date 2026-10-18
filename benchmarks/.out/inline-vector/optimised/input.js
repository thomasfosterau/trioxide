// Pass 5 (function inlining): small vector helpers and methods called in a hot loop.
class Vec2 {
	x;
	y;
	constructor(x, y) {
		this.x = x;
		this.y = y;
	}
	dot(other) {
		return this.x * other.x + this.y * other.y;
	}
	cross(other) {
		return this.x * other.y - this.y * other.x;
	}
}
const points = [];
for (let i = 0; i < 1024; i++) {
	points.push(new Vec2(i % 17 - 8, i % 13 - 6));
}
export function bench() {
	let total = 0;
	for (let i = 1; i < points.length; i++) {
		let v, a_1, b_1;
		const a = points[i - 1];
		const b = points[i];
		const t = (v = (a.x * b.x + a.y * b.y) / 100, v < 0 ? 0 : v > 1 ? 1 : v);
		total += (a_1 = a.x * b.y - a.y * b.x, b_1 = b.x * b.x + b.y * b.y, a_1 + (b_1 - a_1) * t);
	}
	return total;
}
//# sourceMappingURL=input.js.map
