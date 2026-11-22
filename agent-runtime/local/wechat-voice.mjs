import { decode, getDuration, isSilk } from "silk-wasm";

const sampleRate = 16_000;
const maxInput = 2 * 1024 * 1024;
const maxPcm = 60 * sampleRate * 2;

async function readSilk() {
  const chunks = [];
  let length = 0;
  for await (const chunk of process.stdin) {
    length += chunk.length;
    if (length > maxInput) throw Error("invalid input size");
    chunks.push(chunk);
  }
  const silk = Buffer.concat(chunks, length);
  if (!isSilk(silk)) throw Error("invalid SILK signature");
  const duration = getDuration(silk);
  if (!(duration > 0 && duration <= 60_100)) throw Error("invalid SILK duration");
  return silk;
}

try {
  const { data } = await decode(await readSilk(), sampleRate);
  if (!data.byteLength || data.byteLength % 2 !== 0 || data.byteLength > maxPcm)
    throw Error("invalid PCM size");
  const header = Buffer.alloc(44);
  header.write("RIFF", 0);
  header.writeUInt32LE(36 + data.byteLength, 4);
  header.write("WAVEfmt ", 8);
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(1, 22);
  header.writeUInt32LE(sampleRate, 24);
  header.writeUInt32LE(sampleRate * 2, 28);
  header.writeUInt16LE(2, 32);
  header.writeUInt16LE(16, 34);
  header.write("data", 36);
  header.writeUInt32LE(data.byteLength, 40);
  process.stdout.write(header);
  process.stdout.write(data);
} catch {
  process.exitCode = 1;
}
