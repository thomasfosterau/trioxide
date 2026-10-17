// Launches Chromium for the browser tests without downloading one: the Playwright-managed
// browser if present (PLAYWRIGHT_BROWSERS_PATH), else CHROMIUM_PATH or a system Chrome.

import { existsSync } from 'node:fs';

import { chromium } from 'playwright-core';

export function chromiumPath() {
  let managed;
  try {
    managed = chromium.executablePath();
  } catch {
    managed = undefined;
  }
  const candidates = [
    process.env.CHROMIUM_PATH,
    managed,
    '/usr/bin/google-chrome',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  ];
  const found = candidates.find((candidate) => candidate && existsSync(candidate));
  if (!found) throw new Error('no Chromium found; set CHROMIUM_PATH');
  return found;
}

export function launchChromium() {
  return chromium.launch({ executablePath: chromiumPath(), args: ['--no-sandbox'] });
}
