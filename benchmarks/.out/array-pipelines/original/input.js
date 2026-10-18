// Array pipelines (PLAN §4 pass 6): chains of `map`/`filter`/`reduce` and searches over arrays
// the program builds.
const readings = [];
for (let i = 0; i < 2e4; i++) readings.push(i * 7919 % 1e3 / 10);
const orders = [];
for (let i = 0; i < 5e3; i++) {
	orders.push({
		id: i,
		price: i % 97 + .5,
		quantity: i % 7 + 1
	});
}
function summary() {
	const total = readings.map((x) => x * 1.8 + 32).filter((f) => f > 50).reduce((sum, f) => sum + f, 0);
	const anyHot = readings.some((x) => x > 99.5);
	const firstCold = readings.findIndex((x) => x < .5);
	return total + (anyHot ? 1 : 0) + firstCold;
}
function revenue() {
	const big = orders.filter((order) => order.quantity > 3).map((order) => order.price * order.quantity);
	let sum = 0;
	for (let i = 0; i < big.length; i++) sum += big[i];
	return sum;
}
export function bench() {
	return summary() + revenue();
}
export function main() {
	console.log(bench());
}
