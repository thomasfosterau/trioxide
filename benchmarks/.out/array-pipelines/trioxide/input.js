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
	let total = 0;
	for (let i = 0, length = readings.length; i < length; i++) {
		const x = readings[i];
		const f = x * 1.8 + 32;
		if (!(f > 50)) continue;
		total = total + f;
	}
	let anyHot = false;
	for (let i = 0, length = readings.length; i < length; i++) {
		const x = readings[i];
		if (x > 99.5) {
			anyHot = true;
			break;
		}
	}
	let firstCold = -1;
	for (let i = 0, length = readings.length; i < length; i++) {
		const x = readings[i];
		if (x < .5) {
			firstCold = i;
			break;
		}
	}
	return total + (anyHot ? 1 : 0) + firstCold;
}
function revenue() {
	const big = [];
	for (let i = 0, length = orders.length; i < length; i++) {
		const order = orders[i];
		if (!(order.quantity > 3)) continue;
		big.push(order.price * order.quantity);
	}
	let sum = 0;
	for (let i = 0, bigLength = big.length; i < bigLength; i++) sum += big[i];
	return sum;
}
export function bench() {
	return summary() + revenue();
}
export function main() {
	console.log(bench());
}
//# sourceMappingURL=input.js.map
