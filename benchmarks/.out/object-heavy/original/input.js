// Hidden-class stabilisation (PLAN §4 pass 10): class instances that get properties late or
// only sometimes, and lose them with `delete`.
class Particle {
	x;
	y;
	constructor(x, y) {
		this.x = x;
		this.y = y;
		if (x % 3 === 0) this.charge = 1;
		if (y % 5 === 0) this.mass = 2;
	}
}
function energy(particles) {
	let e = 0;
	for (let i = 0; i < particles.length; i++) {
		const p = particles[i];
		e += p.x * (p.charge ?? 0) + p.y * (p.mass ?? 1) + (p.vx ?? 0);
	}
	return e;
}
function simulate() {
	const particles = [];
	for (let i = 0; i < 2e3; i++) {
		const p = new Particle(i, i * 2);
		if (i % 2 === 0) p.vx = i;
		particles.push(p);
	}
	let total = 0;
	for (let step = 0; step < 20; step++) {
		total += energy(particles);
		const p = particles[step];
		delete p.vx;
	}
	return total;
}
export function bench() {
	return simulate();
}
export function main() {
	console.log(bench());
}
