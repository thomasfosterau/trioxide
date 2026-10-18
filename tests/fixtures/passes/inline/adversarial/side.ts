// Runs code when loaded: the import must stay even once its constant is inlined.
console.log("side loaded");
export const SIDE = 42;
