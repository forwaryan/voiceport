// Synthetic Sherpa library for app-local admission and native bridge tests.
#include "c-api.h"

extern "C" int vp_fixture_ort();

struct SherpaOnnxOnlineRecognizer {};
struct SherpaOnnxOnlineStream {};
using Online = SherpaOnnxOnlineRecognizer;
using Stream = SherpaOnnxOnlineStream;
using Offline = SherpaOnnxOfflineRecognizer;
using OfflineStream = SherpaOnnxOfflineStream;

const char *SherpaOnnxGetVersionStr() {
  return vp_fixture_ort() == 1 ? FIXTURE_ABI : nullptr;
}

const Online *SherpaOnnxCreateOnlineRecognizer(const SherpaOnnxOnlineRecognizerConfig *) {
  static const Online recognizer;
  return &recognizer;
}
const Stream *SherpaOnnxCreateOnlineStream(const Online *) {
  static const Stream stream;
  return &stream;
}
void SherpaOnnxDestroyOnlineRecognizer(const Online *) {}
void SherpaOnnxDestroyOnlineStream(const Stream *) {}
void SherpaOnnxOnlineStreamAcceptWaveform(const Stream *, int32_t, const float *, int32_t) {}
int32_t SherpaOnnxIsOnlineStreamReady(const Online *, const Stream *) { return 0; }
void SherpaOnnxDecodeOnlineStream(const Online *, const Stream *) {}
const char *SherpaOnnxGetOnlineStreamResultAsJson(const Online *, const Stream *) {
  return R"({"text":"synthetic"})";
}
void SherpaOnnxDestroyOnlineStreamResultJson(const char *) {}
void SherpaOnnxOnlineStreamInputFinished(const Stream *) {}
int32_t SherpaOnnxOnlineStreamIsEndpoint(const Online *, const Stream *) { return 0; }
void SherpaOnnxOnlineStreamReset(const Online *, const Stream *) {}
void SherpaOnnxOnlineStreamSetOption(const Stream *, const char *, const char *) {}

// The production bridge also resolves the offline exports for streaming models.
const Offline *SherpaOnnxCreateOfflineRecognizer(const SherpaOnnxOfflineRecognizerConfig *) {
  return nullptr;
}
void SherpaOnnxDestroyOfflineRecognizer(const Offline *) {}
const OfflineStream *SherpaOnnxCreateOfflineStream(const Offline *) { return nullptr; }
void SherpaOnnxDestroyOfflineStream(const OfflineStream *) {}
void SherpaOnnxAcceptWaveformOffline(const OfflineStream *, int32_t, const float *, int32_t) {}
#ifndef FIXTURE_OMIT_OFFLINE_DECODE
void SherpaOnnxDecodeOfflineStream(const Offline *, const OfflineStream *) {}
#endif
const char *SherpaOnnxGetOfflineStreamResultAsJson(const OfflineStream *) { return nullptr; }
void SherpaOnnxDestroyOfflineStreamResultJson(const char *) {}
