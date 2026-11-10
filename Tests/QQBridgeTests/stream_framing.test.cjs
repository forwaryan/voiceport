'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { inspectStreamChunks } = require('../../scripts/qq_bridge/stream_framing.cjs');

// Synthetic opaque payloads. None of these fixtures are encoded speech.
const header = Buffer.from('\x02#!SILK_V3', 'ascii');
function frame(payload) {
  const bytes = Buffer.from(payload);
  const length = Buffer.alloc(2);
  length.writeInt16LE(bytes.length);
  return Buffer.concat([length, bytes]);
}
const first = payload => Buffer.concat([header, frame(payload)]);

test('a first chunk and headerless continuation preserve encoded byte offsets', () => {
  const chunks = [first([7, 8, 9]), Buffer.concat([frame([10]), frame([11, 12])])];
  const originals = chunks.map(chunk => Buffer.from(chunk));
  const result = inspectStreamChunks(chunks);
  assert.equal(result.frame_count, 3);
  assert.equal(result.payload_bytes, 6);
  assert.equal(result.total_bytes, 22);
  assert.deepEqual(result.chunks, [
    { index: 0, byte_offset: 0, file_size: 15, frame_count: 1 },
    { index: 1, byte_offset: 15, file_size: 7, frame_count: 2 },
  ]);
  assert.equal(result.transcription_verified, false);
  assert.equal(result.diagnostic_only, true);
  assert.deepEqual(chunks, originals);
  assert.deepEqual(Object.keys(result).sort(), [
    'chunks', 'diagnostic_only', 'frame_count', 'payload_bytes',
    'structurally_valid', 'total_bytes', 'transcription_verified',
  ]);
});

test('independently headed chunks and missing or damaged first headers are rejected', () => {
  assert.throws(() => inspectStreamChunks([first([1]), first([2])]), /truncated_frame_payload/);
  assert.throws(() => inspectStreamChunks([frame([1])]), /missing_stream_header/);
  const damaged = first([1]);
  damaged[0] = 3;
  assert.throws(() => inspectStreamChunks([damaged]), /missing_stream_header/);
  assert.throws(() => inspectStreamChunks([header]), /stream_has_no_frames/);
});

test('opaque continuation payloads may collide with the stream header signature', () => {
  const payload = Buffer.alloc(0x2302, 9);
  Buffer.from('!SILK_V3').copy(payload);
  const continuation = frame(payload);
  assert.ok(continuation.subarray(0, header.length).equals(header));
  const split = inspectStreamChunks([first([1]), continuation]);
  const joined = inspectStreamChunks([Buffer.concat([first([1]), continuation])]);
  assert.equal(split.frame_count, joined.frame_count);
  assert.equal(split.payload_bytes, joined.payload_bytes);
  assert.equal(split.total_bytes, joined.total_bytes);
});

test('truncated lengths, cross-file frames, zero lengths and footer markers fail', () => {
  assert.throws(() => inspectStreamChunks([Buffer.concat([header, Buffer.from([1])])]), /truncated_frame_length/);
  assert.throws(() => inspectStreamChunks([
    Buffer.concat([header, Buffer.from([2, 0, 9])]), Buffer.from([10]),
  ]), /truncated_frame_payload/);
  for (const invalidLength of [Buffer.from([0, 0]), Buffer.from([255, 255]), Buffer.from([0, 128])]) {
    assert.throws(() => inspectStreamChunks([Buffer.concat([header, invalidLength])]), /invalid_frame_length/);
  }
});

test('local input budgets reject invalid types, oversized buffers and too many records', () => {
  for (const invalid of [null, [], [new Uint8Array(4)], [Buffer.alloc(0)], Array(4097).fill(first([1]))]) {
    assert.throws(() => inspectStreamChunks(invalid), /invalid_chunk/);
  }
  assert.throws(() => inspectStreamChunks([Buffer.alloc(16 * 1024 * 1024 + 1)]), /stream_byte_limit/);
  const manyFrames = Buffer.alloc(3 * 65537);
  for (let offset = 0; offset < manyFrames.length; offset += 3) {
    manyFrames[offset] = 1;
  }
  assert.throws(() => inspectStreamChunks([Buffer.concat([header, manyFrames])]), /stream_frame_limit/);
});

test('metadata-only result makes no SILK validity or duration claim', () => {
  const result = inspectStreamChunks([first(Buffer.from('PRIVATE_SYNTHETIC_BYTES'))]);
  assert.equal(JSON.stringify(result).includes('PRIVATE_SYNTHETIC_BYTES'), false);
  assert.equal('duration_ms' in result, false);
  assert.equal('sample_rate' in result, false);
  assert.equal('codec_verified' in result, false);
});
