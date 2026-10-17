#!/usr/bin/env node
// The `trioxide` command. It runs the Rust CLI inside the napi binding, so `npx trioxide` and
// the standalone binary are the same implementation.

import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const binding = require('../binding/index.cjs');

process.exitCode = binding.runCli(['trioxide', ...process.argv.slice(2)]);
