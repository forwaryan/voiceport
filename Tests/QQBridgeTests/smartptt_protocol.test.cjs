'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { buildSmartPttRequest, createTranscriptState } = require('../../scripts/qq_bridge/smartptt_protocol.cjs');
const id = '0123456789abcdef0123456789abcd';
const other = 'abcdef0123456789abcdef01234567';

// All paths, task IDs and callback text below are synthetic. No QQ is called.
test('chunk metadata preserves encoded offsets and QQ voice-input chat type', () => {
  const chunk = { filePath: '/synthetic/chunk_0001.silk', chunkIndex: 1,
    chunkOffsetBytes: 317, durationMs: 2000, startMs: 2000, isLast: 1 };
  assert.deepEqual(buildSmartPttRequest(id, chunk, { removePeriod: true }), {
    taskId: id,
    sliceParam: { chatType: 7, sliceIndex: 1, filePath: chunk.filePath,
      voiceOffset: 317, isEnd: true, removePeriod: true },
  });
  assert.equal(chunk.chunkOffsetBytes, 317);
  assert.equal(buildSmartPttRequest(id, { ...chunk, isLast: 2 }).sliceParam.isEnd, false);
  assert.equal(buildSmartPttRequest(id, { ...chunk, isLast: 0 }).sliceParam.isEnd, false);
  assert.throws(() => buildSmartPttRequest(id, { ...chunk, isLast: true }), /invalid_is_last/);
  for (const value of [-1, 0x100000000, 1.5, NaN]) {
    assert.throws(() => buildSmartPttRequest(id, { ...chunk, chunkOffsetBytes: value }), /invalid_chunk_offset/);
  }
});

test('revised partials replace one segment and completed segments accumulate', () => {
  const state = createTranscriptState(id);
  assert.equal(state.update(id, '你', false).text, '你');
  assert.equal(state.update(id, '你好', false).text, '你好');
  assert.equal(state.update(id, '你好，', true).text, '你好，');
  assert.equal(state.update(id, '世', false).text, '你好，世');
  assert.equal(state.update(id, '世界', false).text, '你好，世界');
  assert.deepEqual(state.finish(id, 0, ''), { status: 'finished', text: '你好，世界' });
});

test('segment completion does not finish the task and stale callbacks cannot alter it', () => {
  const state = createTranscriptState(id);
  assert.deepEqual(state.update(other, 'wrong task', true), { status: 'ignored' });
  assert.deepEqual(state.finish(other, 0, ''), { status: 'ignored' });
  assert.equal(state.update(id, 'A', true).status, 'partial');
  assert.equal(state.update(id, 'B', true).text, 'AB');
  assert.deepEqual(state.finish(id, 0, ''), { status: 'finished', text: 'AB' });
  assert.deepEqual(state.update(id, 'late', true), { status: 'ignored' });
  assert.deepEqual(state.finish(id, 0, ''), { status: 'ignored' });
});

test('failure completion cannot turn a partial transcript into a successful final', () => {
  const state = createTranscriptState(id);
  state.update(id, 'unfinished', false);
  assert.deepEqual(state.finish(id, -7, 'synthetic failure'), {
    status: 'failed', errorCode: -7, error: 'synthetic failure',
  });
  assert.deepEqual(state.finish(id, 0, ''), { status: 'ignored' });
});

test('invalid updates and local text budget failures preserve prior transcript', () => {
  const state = createTranscriptState(id);
  state.update(id, 'saved', true);
  assert.throws(() => state.update(id, 'x', 1), /invalid_text_update/);
  assert.throws(() => state.update(id, 'x'.repeat(1024 * 1024), false), /transcript_text_limit/);
  assert.throws(() => state.finish(id, 0x80000000, ''), /invalid_finish/);
  assert.deepEqual(state.finish(id, 0, ''), { status: 'finished', text: 'saved' });
  assert.throws(() => createTranscriptState(''), /invalid_task_id/);
});
