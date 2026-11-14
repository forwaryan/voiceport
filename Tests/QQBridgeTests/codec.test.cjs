'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { MAX_WAV, parseWav, decodeWav, normalizeSilk, encodeWav } = require('../../scripts/qq_bridge/codec.cjs');

// Synthetic PCM and opaque SILK records only; none of these fixtures is speech.
function chunk(name, data) {
  const prefix = Buffer.alloc(8);
  prefix.write(name, 0, 'ascii');
  prefix.writeUInt32LE(data.length, 4);
  return Buffer.concat([prefix, data, Buffer.alloc(data.length % 2)]);
}
function riff(chunks) {
  const body = Buffer.concat([Buffer.from('WAVE'), ...chunks]);
  const prefix = Buffer.alloc(8);
  prefix.write('RIFF');
  prefix.writeUInt32LE(body.length, 4);
  return Buffer.concat([prefix, body]);
}
function format() {
  const fmt = Buffer.alloc(16);
  fmt.writeUInt16LE(1, 0);
  fmt.writeUInt16LE(1, 2);
  fmt.writeUInt32LE(16000, 4);
  fmt.writeUInt32LE(32000, 8);
  fmt.writeUInt16LE(2, 12);
  fmt.writeUInt16LE(16, 14);
  return fmt;
}
const wav = (pcm = Buffer.alloc(2)) => riff([chunk('fmt ', format()), chunk('data', pcm)]);
const header = Buffer.from('\x02#!SILK_V3', 'ascii');
function frame(payload) {
  const size = Buffer.alloc(2);
  size.writeInt16LE(payload.length);
  return Buffer.concat([size, Buffer.from(payload)]);
}
const silk = payload => Buffer.concat([header, frame(payload)]);

test('WAV parser accepts the Rust PCM16 mono 16 kHz layout including the exact 180 second limit', () => {
  const pcm = Buffer.from([0, 128, 255, 127, 0, 0]);
  const bytes = wav(pcm);
  const original = Buffer.from(bytes);
  assert.deepEqual(parseWav(bytes), { pcm, sampleRate: 16000, sampleCount: 3 });
  assert.deepEqual(bytes, original);
  const maximum = wav(Buffer.alloc(180 * 16000 * 2));
  assert.equal(maximum.length, MAX_WAV);
  assert.equal(parseWav(maximum).sampleCount, 180 * 16000);
  assert.throws(() => parseWav(wav(Buffer.alloc(180 * 16000 * 2 + 2))), /invalid_wav/);
  assert.throws(() => parseWav(wav(Buffer.alloc(0))), /invalid_wav/);
});

test('WAV parser checks actual RIFF/chunk lengths, format fields, alignment, and unique ordered data', () => {
  const malformed = [];
  for (const [offset, value, width] of [
    [20, 3, 2], [20, 65534, 2], [22, 2, 2], [24, 44100, 4],
    [28, 16000, 4], [32, 4, 2], [34, 8, 2], [4, 1, 4], [40, 500, 4],
  ]) {
    const bytes = wav();
    bytes[width === 2 ? 'writeUInt16LE' : 'writeUInt32LE'](value, offset);
    malformed.push(bytes);
  }
  const fmt = chunk('fmt ', format());
  const data = chunk('data', Buffer.alloc(2));
  malformed.push(
    Buffer.alloc(44), wav().subarray(0, -1), riff([fmt]), riff([data, fmt]),
    riff([fmt, fmt, data]), riff([fmt, data, data]), riff([fmt, chunk('data', Buffer.alloc(3))]),
    riff([chunk('fmt ', Buffer.alloc(15)), data]),
    riff([fmt, data, Buffer.from([1])]),
    riff([fmt, data, ...Array.from({ length: 127 }, () => chunk('JUNK', Buffer.alloc(0)))]),
  );
  for (const bytes of malformed) assert.throws(() => parseWav(bytes), /invalid_wav/);
  const withPadding = riff([chunk('JUNK', Buffer.from([1, 2, 3])), fmt, data]);
  assert.equal(parseWav(withPadding).sampleCount, 1);
  const extended = riff([chunk('fmt ', Buffer.concat([format(), Buffer.alloc(2)])), data]);
  assert.equal(parseWav(extended).sampleCount, 1);
  const nonzeroExtension = Buffer.concat([format(), Buffer.from([1, 0])]);
  assert.throws(() => parseWav(riff([chunk('fmt ', nonzeroExtension), data])), /invalid_wav/);
});

test('base64 must be canonical, bounded, and decode to a validated WAV', () => {
  const bytes = wav();
  const canonical = bytes.toString('base64');
  assert.deepEqual(decodeWav(canonical), bytes);
  const padBits = canonical.slice(0, -3) + 'B==';
  assert.deepEqual(Buffer.from(padBits, 'base64'), bytes, 'permissive Node decoder accepts nonzero pad bits');
  for (const invalid of [null, '', canonical + '\n', canonical.slice(0, -1), padBits,
    '-' + canonical.slice(1), '*' + canonical, 'AAAA', 'A'.repeat(4 * Math.ceil(MAX_WAV / 3) + 4)]) {
    assert.throws(() => decodeWav(invalid), /invalid_wav/);
  }
});

test('SILK normalization preserves its header and positive frames, removing only a standalone -1 footer', () => {
  const bytes = silk([1, 255, 255]);
  const original = Buffer.from(bytes);
  assert.deepEqual(normalizeSilk(bytes), bytes, 'ff ff in a payload is not a footer');
  assert.deepEqual(normalizeSilk(Buffer.concat([bytes, Buffer.from([255, 255])])), bytes);
  assert.deepEqual(bytes, original);
  const storage = Buffer.concat([Buffer.from([9]), bytes, Buffer.from([9])]);
  const view = new Uint8Array(storage.buffer, storage.byteOffset + 1, bytes.length);
  const normalized = normalizeSilk(view);
  assert.deepEqual(normalized, bytes);
  assert.ok(normalized.subarray(0, header.length).equals(header));
  normalized[0] = 9;
  assert.equal(storage[1], 2, 'normalization returns independent storage');
});

test('SILK normalization rejects empty, damaged, nonpositive, incomplete and repeated-footer streams', () => {
  const bytes = silk([1]);
  for (const invalid of [null, {}, new Int16Array(4), Buffer.alloc(0), header,
    Buffer.concat([header, Buffer.from([255, 255])]),
    Buffer.concat([header, Buffer.from([0, 0])]),
    Buffer.concat([header, Buffer.from([2, 0, 1])]),
    Buffer.concat([bytes, Buffer.from([255])]),
    Buffer.concat([bytes, Buffer.from([255, 255, 255, 255])]),
    Buffer.concat([bytes, Buffer.from([254, 255])]),
    bytes.subarray(1), Buffer.alloc(3_000_003)]) {
    assert.throws(() => normalizeSilk(invalid));
  }
});

test('encoding validates before lazily loading silk-wasm and uses its reviewed encode API', async () => {
  let loads = 0;
  const calls = [];
  const encoded = silk([7, 8]);
  const loadCodec = () => {
    loads++;
    return { async encode(...args) { calls.push(args); return { data: new Uint8Array(Buffer.concat([encoded, Buffer.from([255, 255])])), duration: 20 }; } };
  };
  await assert.rejects(encodeWav(Buffer.alloc(44), { loadCodec }), /invalid_wav/);
  assert.equal(loads, 0);
  const bytes = wav(Buffer.alloc(640));
  assert.deepEqual(await encodeWav(bytes, { loadCodec }), encoded);
  assert.equal(loads, 1);
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], bytes);
  assert.equal(calls[0][1], 16000);
  await assert.rejects(encodeWav(bytes, { loadCodec: () => ({ encode: async () => ({ data: Buffer.alloc(0) }) }) }));
  await assert.rejects(encodeWav(bytes, { loadCodec: () => { throw new Error('missing synthetic codec'); } }));
});
