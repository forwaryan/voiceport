'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { splitStream, inspectStreamChunks } = require('../../scripts/qq_bridge/stream_framing.cjs');
const header = Buffer.from('\x02#!SILK_V3');
const frame = Buffer.from([2, 0, 17, 18]);
const data = count => Buffer.concat([header, ...Array(count).fill(frame)]);
test('frame-count boundaries retain exactly one header and every encoded byte', () => {
  for (const count of [1, 199, 200, 201, 400, 401, 9000]) {
    const original = data(count);
    const chunks = splitStream(original);
    assert.equal(chunks.length, Math.ceil(count / 200));
    assert.deepEqual(Buffer.concat(chunks), original);
    const info = inspectStreamChunks(chunks);
    assert.equal(info.frame_count, count);
    assert.equal(info.total_bytes, original.length);
    assert.ok(info.chunks.every(chunk => chunk.frame_count <= 200));
    assert.ok(chunks.every(chunk => chunk.length <= 16384));
  }
});
test('byte budget cuts only between complete frames', () => {
  const largeFrame = Buffer.concat([Buffer.from([200, 0]), Buffer.alloc(200, 37)]);
  const original = Buffer.concat([header, ...Array(450).fill(largeFrame)]);
  const chunks = splitStream(original);
  assert.ok(chunks.every(chunk => chunk.length <= 16384));
  assert.deepEqual(Buffer.concat(chunks), original);
  assert.equal(inspectStreamChunks(chunks).frame_count, 450);
});
test('a large continuation frame does not reserve another stream header', () => {
  const payload = Buffer.alloc(16380, 37);
  const length = Buffer.alloc(2);
  length.writeInt16LE(payload.length);
  const original = Buffer.concat([header, Buffer.from([1, 0, 17]), length, payload]);
  const chunks = splitStream(original);
  assert.deepEqual(chunks.map(chunk => chunk.length), [13, 16382]);
  assert.deepEqual(Buffer.concat(chunks), original);
  assert.equal(inspectStreamChunks(chunks).frame_count, 2);
  assert.throws(() => splitStream(Buffer.concat([header, length, payload])),
    /frame_exceeds_slice_budget/);
});

test('damaged input and an indivisible oversized frame are rejected', () => {
  for (const original of [Buffer.from('bad'), header, Buffer.concat([header, Buffer.from([2, 0, 1])]),
    Buffer.concat([header, Buffer.from([0, 64]), Buffer.alloc(16384)])]) {
    assert.throws(() => splitStream(original));
  }
});
