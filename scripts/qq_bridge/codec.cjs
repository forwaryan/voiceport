'use strict';

const { inspectStreamChunks } = require('./stream_framing.cjs');
const MAX_PCM = 180 * 16000 * 2;
const MAX_WAV = MAX_PCM + 44;
const MAX_SILK = 3_000_000;

function parseWav(wav) {
  if (!Buffer.isBuffer(wav) || wav.length < 44 || wav.length > MAX_WAV ||
      wav.toString('latin1', 0, 4) !== 'RIFF' ||
      wav.toString('latin1', 8, 12) !== 'WAVE' || wav.readUInt32LE(4) !== wav.length - 8) {
    throw new Error('invalid_wav');
  }
  let format = false;
  let pcm;
  let chunks = 0;
  for (let offset = 12; offset < wav.length;) {
    if (++chunks > 128 || wav.length - offset < 8) throw new Error('invalid_wav');
    const name = wav.toString('latin1', offset, offset + 4);
    const size = wav.readUInt32LE(offset + 4);
    const start = offset + 8;
    const end = start + size;
    const next = end + (size % 2);
    if (next > wav.length) throw new Error('invalid_wav');
    if (name === 'fmt ') {
      if (format || (size !== 16 && size !== 18) ||
          (size === 18 && wav.readUInt16LE(start + 16) !== 0) ||
          wav.readUInt16LE(start) !== 1 || wav.readUInt16LE(start + 2) !== 1 ||
          wav.readUInt32LE(start + 4) !== 16000 || wav.readUInt32LE(start + 8) !== 32000 ||
          wav.readUInt16LE(start + 12) !== 2 || wav.readUInt16LE(start + 14) !== 16) {
        throw new Error('invalid_wav');
      }
      format = true;
    } else if (name === 'data') {
      if (!format || pcm || !size || size % 2 || size > MAX_PCM) throw new Error('invalid_wav');
      pcm = wav.subarray(start, end);
    }
    offset = next;
  }
  if (!format || !pcm) throw new Error('invalid_wav');
  return { pcm, sampleRate: 16000, sampleCount: pcm.length / 2 };
}

function decodeWav(base64) {
  if (typeof base64 !== 'string' || !base64.length || base64.length % 4 ||
      base64.length > 4 * Math.ceil(MAX_WAV / 3)) throw new Error('invalid_wav');
  const wav = Buffer.from(base64, 'base64');
  // Buffer's decoder is permissive; round-tripping rejects whitespace, URL-safe
  // alphabet, missing padding and nonzero pad bits as well as arbitrary junk.
  if (wav.toString('base64') !== base64) throw new Error('invalid_wav');
  parseWav(wav);
  return wav;
}

function normalizeSilk(data) {
  if (!(data instanceof Uint8Array) || !data.byteLength || data.byteLength > MAX_SILK + 2) {
    throw new Error('invalid_silk');
  }
  let silk = Buffer.from(data.buffer, data.byteOffset, data.byteLength);
  try {
    inspectStreamChunks([silk]);
  } catch {
    // Only remove a standalone signed -1 record at the exact end, after all
    // preceding positive frame records validate. A payload ending ff ff stays.
    if (silk.length < 2 || silk.readInt16LE(silk.length - 2) !== -1) throw new Error('invalid_silk');
    silk = silk.subarray(0, -2);
    inspectStreamChunks([silk]);
  }
  if (silk.length > MAX_SILK) throw new Error('invalid_silk');
  return Buffer.from(silk);
}

async function encodeWav(wav, { loadCodec = () => require('silk-wasm') } = {}) {
  parseWav(wav);
  // silk-wasm 3.7.1: encode(Buffer | Uint8Array, sampleRate) -> { data, duration }.
  // This is deliberately lazy: invalid input never loads the WASM dependency.
  const result = await loadCodec().encode(wav, 16000);
  return normalizeSilk(result && result.data);
}

module.exports = { MAX_WAV, decodeWav, parseWav, normalizeSilk, encodeWav };
