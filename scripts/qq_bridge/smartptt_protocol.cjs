'use strict';

// Data-only helpers derived from QQ 7.0.1-52892 AudioSDK/VoiceInputStore
// static instructions. No native module, files, IPC, capture or upload.
// These helpers do not establish a compatible QQ entry or real recognition.
const UINT32_MAX = 0xffffffff;
const MAX_TEXT_LENGTH = 1024 * 1024; // Local memory budget, not a QQ limit.

function uint32(value, name) {
  if (!Number.isInteger(value) || value < 0 || value > UINT32_MAX) {
    throw new TypeError(`invalid_${name}`);
  }
  return value;
}

function taskId(value) {
  // QQ generates randomBytes(15).toString('hex'). This is the observed
  // caller convention, not a claim that the server requires this syntax.
  if (typeof value !== 'string' || !/^[0-9a-f]{30}$/.test(value)) {
    throw new TypeError('invalid_task_id');
  }
  return value;
}

function buildSmartPttRequest(id, chunk, { removePeriod = false } = {}) {
  taskId(id);
  if (!chunk || typeof chunk.filePath !== 'string' || chunk.filePath.length === 0) {
    throw new TypeError('invalid_file_path');
  }
  if (typeof removePeriod !== 'boolean') throw new TypeError('invalid_remove_period');
  uint32(chunk.isLast, 'is_last');
  return {
    taskId: id,
    sliceParam: {
      chatType: 7,
      sliceIndex: uint32(chunk.chunkIndex, 'chunk_index'),
      filePath: chunk.filePath,
      voiceOffset: uint32(chunk.chunkOffsetBytes, 'chunk_offset_bytes'),
      isEnd: chunk.isLast === 1,
      removePeriod,
    },
  };
}

function createTranscriptState(id) {
  taskId(id);
  let confirmed = '';
  let current = '';
  let closed = false;
  const ignored = () => ({ status: 'ignored' });
  return Object.freeze({
    update(callbackId, text, segFinished) {
      if (closed || callbackId !== id) return ignored();
      if (typeof text !== 'string' || typeof segFinished !== 'boolean') {
        throw new TypeError('invalid_text_update');
      }
      if (confirmed.length + text.length > MAX_TEXT_LENGTH) {
        throw new RangeError('transcript_text_limit');
      }
      // Each update replaces the current segment. A completed segment is
      // committed once; the next update starts a new segment.
      current = text;
      if (segFinished) {
        confirmed += current;
        current = '';
      }
      return { status: 'partial', text: confirmed + current, segmentFinished: segFinished };
    },
    finish(callbackId, errorCode, errMsg) {
      if (closed || callbackId !== id) return ignored();
      if (!Number.isInteger(errorCode) || errorCode < -0x80000000 || errorCode > 0x7fffffff ||
          typeof errMsg !== 'string' || errMsg.length > MAX_TEXT_LENGTH) {
        throw new TypeError('invalid_finish');
      }
      closed = true;
      if (errorCode !== 0) {
        return { status: 'failed', errorCode, error: errMsg };
      }
      return { status: 'finished', text: confirmed + current };
    },
  });
}

module.exports = { buildSmartPttRequest, createTranscriptState };
