// Per-device summaries and alerts over decoded readings.

import type { Reading } from "./readings.js";

export interface Summary {
  device: number;
  count: number;
  mean: number;
  min: number;
  max: number;
  dewPoint: number;
}

function dewPoint(celsius: number, humidity: number): number {
  const a = 17.27;
  const b = 237.7;
  const gamma = (a * celsius) / (b + celsius) + Math.log(Math.max(humidity, 1) / 100);
  return (b * gamma) / (a - gamma);
}

function summarise(device: number, readings: Reading[]): Summary {
  const acc = { count: 0, sum: 0, min: Infinity, max: -Infinity, humidity: 0 };
  for (const r of readings) {
    if (r.device !== device) continue;
    acc.count++;
    acc.sum += r.celsius;
    acc.humidity += r.humidity;
    if (r.celsius < acc.min) acc.min = r.celsius;
    if (r.celsius > acc.max) acc.max = r.celsius;
  }
  const mean = acc.count === 0 ? 0 : acc.sum / acc.count;
  return { device, count: acc.count, mean, min: acc.min, max: acc.max, dewPoint: dewPoint(mean, acc.humidity / Math.max(acc.count, 1)) };
}

export function summaries(readings: Reading[], devices: number): Summary[] {
  const result: Summary[] = [];
  for (let device = 0; device < devices; device++) result.push(summarise(device, readings));
  return result;
}

export function alerts(readings: Reading[], limit: number): string[] {
  return readings
    .filter((r) => r.celsius > limit)
    .map((r) => `${r.device}@${r.time}`)
    .slice(0, 5);
}

export function hottest(list: Summary[]): number[] {
  return list
    .filter((s) => s.count > 0)
    .sort((a, b) => b.mean - a.mean || a.device - b.device)
    .slice(0, 3)
    .map((s) => s.device);
}
