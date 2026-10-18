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
function lerp(a, b, t) {
	return a + (b - a) * t;
}
function clamp(v, lo, hi) {
	return v < lo ? lo : v > hi ? hi : v;
}
const points = [];
for (let i = 0; i < 1024; i++) {
	points.push(new Vec2(i % 17 - 8, i % 13 - 6));
}
export function bench() {
	let total = 0;
	for (let i = 1; i < points.length; i++) {
		const a = points[i - 1];
		const b = points[i];
		const t = clamp(a.dot(b) / 100, 0, 1);
		total += lerp(a.cross(b), b.dot(b), t);
	}
	return total;
}
