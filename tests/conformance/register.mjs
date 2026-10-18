// Registers resolve.mjs (`node --import ./register.mjs …`).
import { register } from 'node:module';

register('./resolve.mjs', import.meta.url);
