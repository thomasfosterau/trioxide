// Schedules work while modules are still evaluating: top-level await in the glue would let it
// run before the modules importing the island. (No timers: the harness awaits instantiation
// before main(), which lets them fire.)
console.log("sibling body");
queueMicrotask(() => console.log("microtask"));
Promise.resolve()
  .then(() => undefined)
  .then(() => console.log("later microtask"));
export const loaded = true;
