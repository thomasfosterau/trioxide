// Sensor readings: a device id, a raw ADC value, and a timestamp, decoded into records.

export interface Reading {
  device: number;
  time: number;
  celsius: number;
  humidity: number;
}

function lcg(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 4294967296;
  };
}

/** Raw packets as the gateway sends them: 8 bytes per reading. */
export function packets(count: number, seed: number): Uint8Array {
  const random = lcg(seed);
  const bytes = new Uint8Array(count * 8);
  for (let i = 0; i < count; i++) {
    const device = Math.floor(random() * 64);
    const raw = 1800 + Math.floor(Math.sin(i / 40 + device) * 600 + random() * 120);
    const humidity = Math.floor(random() * 1000);
    bytes[i * 8] = device;
    bytes[i * 8 + 1] = raw >> 8;
    bytes[i * 8 + 2] = raw & 255;
    bytes[i * 8 + 3] = humidity >> 8;
    bytes[i * 8 + 4] = humidity & 255;
    bytes[i * 8 + 5] = (i >> 16) & 255;
    bytes[i * 8 + 6] = (i >> 8) & 255;
    bytes[i * 8 + 7] = i & 255;
  }
  return bytes;
}

function clamp(value: number, low: number, high: number): number {
  return value < low ? low : value > high ? high : value;
}

function toCelsius(raw: number): number {
  return clamp((raw / 4096) * 165 - 40, -40, 125);
}

export function decode(bytes: Uint8Array): Reading[] {
  const readings: Reading[] = [];
  for (let at = 0; at + 8 <= bytes.length; at += 8) {
    readings.push({
      device: bytes[at],
      celsius: toCelsius((bytes[at + 1] << 8) | bytes[at + 2]),
      humidity: clamp(((bytes[at + 3] << 8) | bytes[at + 4]) / 10, 0, 100),
      time: (bytes[at + 5] << 16) | (bytes[at + 6] << 8) | bytes[at + 7],
    });
  }
  return readings;
}
