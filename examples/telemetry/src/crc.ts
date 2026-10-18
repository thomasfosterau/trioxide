// CRC-32 (IEEE) a nibble at a time, with a 16-entry table built when the module loads.

function crcNibbleTable(): number[] {
  const table: number[] = [];
  for (let n = 0; n < 16; n++) {
    let c = n;
    for (let k = 0; k < 4; k++) c = c & 1 ? (c >>> 1) ^ 0xedb88320 : c >>> 1;
    table.push(c >>> 0);
  }
  return table;
}

const NIBBLES = crcNibbleTable();

export function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (let i = 0; i < bytes.length; i++) {
    crc = NIBBLES[(crc ^ bytes[i]) & 15] ^ (crc >>> 4);
    crc = NIBBLES[(crc ^ (bytes[i] >>> 4)) & 15] ^ (crc >>> 4);
  }
  return (crc ^ 0xffffffff) >>> 0;
}
