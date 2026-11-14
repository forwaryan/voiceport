'use strict';

// Structural check for the statically recovered AudioFoundation stream format.
// Accepts caller-provided buffers only: no files, native module, encoder or IPC.
// Payload bytes are opaque. Passing this check does not establish valid SILK or ASR.
const HEADER = Buffer.from([2, ...Buffer.from('#!SILK_V3', 'ascii')]);
// Local inspection budgets, not QQ service limits.
const MAX_BYTES = 16 * 1024 * 1024;
const MAX_CHUNKS = 4096;
const MAX_FRAMES = 65536;

function inspectStreamChunks(chunks) {
  if (!Array.isArray(chunks) || chunks.length === 0 || chunks.length > MAX_CHUNKS) {
    throw new TypeError('invalid_chunk_list');
  }
  let totalBytes = 0;
  // Validate the entire input budget before scanning payload framing.
  for (const chunk of chunks) {
    if (!Buffer.isBuffer(chunk) || chunk.length === 0) throw new TypeError('invalid_chunk');
    totalBytes += chunk.length;
    if (totalBytes > MAX_BYTES) throw new RangeError('stream_byte_limit');
  }
  if (chunks[0].length < HEADER.length || !chunks[0].subarray(0, HEADER.length).equals(HEADER)) {
    throw new Error('missing_stream_header');
  }
  let frameCount = 0;
  let payloadBytes = 0;
  let byteOffset = 0;
  const summaries = [];
  for (let index = 0; index < chunks.length; index++) {
    const chunk = chunks[index];
    // Continuations start at a length record. A signature collision can also be
    // a valid opaque frame: rely on complete framing, not payload magic bytes.
    let cursor = index === 0 ? HEADER.length : 0;
    let frames = 0;
    while (cursor < chunk.length) {
      if (chunk.length - cursor < 2) throw new Error('truncated_frame_length');
      const length = chunk.readInt16LE(cursor);
      cursor += 2;
      // No standalone SILK footer was observed in the traced stream writer.
      if (length <= 0) throw new Error('invalid_frame_length');
      if (length > chunk.length - cursor) throw new Error('truncated_frame_payload');
      if (++frameCount > MAX_FRAMES) throw new RangeError('stream_frame_limit');
      frames++;
      payloadBytes += length;
      cursor += length;
    }
    summaries.push({ index, byte_offset: byteOffset, file_size: chunk.length, frame_count: frames });
    byteOffset += chunk.length;
  }
  if (frameCount === 0) throw new Error('stream_has_no_frames');
  return {
    diagnostic_only: true,
    structurally_valid: true,
    transcription_verified: false,
    total_bytes: totalBytes,
    payload_bytes: payloadBytes,
    frame_count: frameCount,
    chunks: summaries,
  };
}

// Keep each native upload small and frame-aligned. Only the first slice owns
// the stream header; continuation offsets count all preceding encoded bytes.
function splitStream(data) {
  inspectStreamChunks([data]);
  const chunks = [];
  let start = 0;
  let cursor = HEADER.length;
  let frames = 0;
  while (cursor < data.length) {
    const next = cursor + 2 + data.readInt16LE(cursor);
    if (frames && (frames === 200 || next - start > 16384)) {
      chunks.push(data.subarray(start, cursor));
      start = cursor;
      frames = 0;
    }
    if (next - start > 16384) {
      throw new RangeError('frame_exceeds_slice_budget');
    }
    cursor = next;
    frames++;
  }
  chunks.push(data.subarray(start));
  return chunks;
}

module.exports = { inspectStreamChunks, splitStream };
