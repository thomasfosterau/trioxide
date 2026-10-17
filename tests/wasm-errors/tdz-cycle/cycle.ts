import { getN } from "./kernel.js";
let early = -1;
try { early = getN(2); } catch (e) { early = -2; }
export { early };
