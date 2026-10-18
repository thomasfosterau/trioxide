import { crc32 } from "./crc.js";
import { decode, packets } from "./readings.js";
import { alerts, hottest, summaries } from "./stats.js";

const BATCH = packets(4000, 2026);

/** Verifies, decodes and summarises one batch of packets: what the gateway does per upload. */
export function run(): string[] {
  const checksum = crc32(BATCH);
  const readings = decode(BATCH);
  const list = summaries(readings, 64);
  return [
    `crc ${checksum.toString(16)}`,
    `readings ${readings.length}`,
    `hottest ${hottest(list).join(",")}`,
    `alerts ${alerts(readings, 60).join(" ")}`,
    `device 7 ${JSON.stringify(list[7])}`,
  ];
}

export function main(): void {
  for (const line of run()) console.log(line);
}
