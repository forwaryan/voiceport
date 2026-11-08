#include "c-api.h"
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <exception>

#define FUNCTIONS(X) \
 X(SherpaOnnxGetVersionStr) \
 X(SherpaOnnxCreateOnlineRecognizer) X(SherpaOnnxDestroyOnlineRecognizer) \
 X(SherpaOnnxCreateOnlineStream) X(SherpaOnnxDestroyOnlineStream) \
 X(SherpaOnnxOnlineStreamAcceptWaveform) X(SherpaOnnxIsOnlineStreamReady) \
 X(SherpaOnnxDecodeOnlineStream) X(SherpaOnnxGetOnlineStreamResultAsJson) \
 X(SherpaOnnxDestroyOnlineStreamResultJson) X(SherpaOnnxOnlineStreamInputFinished) \
 X(SherpaOnnxOnlineStreamIsEndpoint) X(SherpaOnnxOnlineStreamReset) \
 X(SherpaOnnxCreateOfflineRecognizer) X(SherpaOnnxDestroyOfflineRecognizer) \
 X(SherpaOnnxCreateOfflineStream) X(SherpaOnnxDestroyOfflineStream) \
 X(SherpaOnnxAcceptWaveformOffline) X(SherpaOnnxDecodeOfflineStream) \
 X(SherpaOnnxGetOfflineStreamResultAsJson) X(SherpaOnnxDestroyOfflineStreamResultJson)

struct Runtime {
  void *ort = nullptr, *library = nullptr;
  const SherpaOnnxOnlineRecognizer *online = nullptr;
  const SherpaOnnxOnlineStream *stream = nullptr;
  const SherpaOnnxOfflineRecognizer *offline = nullptr;
  char error[1024] = {};
#define FIELD(name) decltype(&::name) name = nullptr;
  FUNCTIONS(FIELD)
#undef FIELD
};
#define CATCH_ERROR(target, capacity) \
  catch (const std::exception &e) { snprintf(target, capacity, "%s", e.what()); } \
  catch (...) { snprintf(target, capacity, "Unknown native inference exception"); }

extern "C" {
void vp_local_close(Runtime *r) {
  if (!r) return;
  if (r->stream) r->SherpaOnnxDestroyOnlineStream(r->stream);
  if (r->online) r->SherpaOnnxDestroyOnlineRecognizer(r->online);
  if (r->offline) r->SherpaOnnxDestroyOfflineRecognizer(r->offline);
  if (r->library) dlclose(r->library);
  if (r->ort) dlclose(r->ort);
  delete r;
}
const char *vp_local_error(Runtime *r) { return r->error; }
Runtime *vp_local_open(const char *directory, const char *encoder,
    const char *decoder, const char *tokens, const char *language,
    int online, char *error, size_t capacity) {
  Runtime *r = nullptr;
  try { r = new Runtime; } CATCH_ERROR(error, capacity)
  if (!r) return nullptr;
  char path[4096];
  if (snprintf(path, sizeof(path), "%s/libonnxruntime.1.23.2.dylib", directory) >= (int)sizeof(path)) goto fail;
  r->ort = dlopen(path, RTLD_NOW | RTLD_GLOBAL);
  if (!r->ort) { snprintf(error, capacity, "ONNX Runtime: %s", dlerror()); goto fail; }
  if (snprintf(path, sizeof(path), "%s/libsherpa-onnx-c-api.dylib", directory) >= (int)sizeof(path)) goto fail;
  r->library = dlopen(path, RTLD_NOW | RTLD_LOCAL);
  if (!r->library) { snprintf(error, capacity, "sherpa-onnx: %s", dlerror()); goto fail; }
#define LOAD(name) \
  r->name = reinterpret_cast<decltype(r->name)>(dlsym(r->library, #name)); \
  if (!r->name) { snprintf(error, capacity, "Missing runtime symbol: %s", #name); goto fail; }
  FUNCTIONS(LOAD)
#undef LOAD
  if (strcmp(r->SherpaOnnxGetVersionStr(), "1.12.37") != 0) {
    snprintf(error, capacity, "Unsupported sherpa-onnx ABI: %s (requires 1.12.37)", r->SherpaOnnxGetVersionStr());
    goto fail;
  }
  try {
    if (online) {
      SherpaOnnxOnlineRecognizerConfig c = {};
      c.feat_config.sample_rate = 16000;
      c.feat_config.feature_dim = 80;
      c.model_config.paraformer.encoder = encoder;
      c.model_config.paraformer.decoder = decoder;
      c.model_config.tokens = tokens;
      c.model_config.num_threads = 2;
      c.model_config.provider = "cpu";
      c.decoding_method = "greedy_search";
      c.enable_endpoint = 1;
      c.rule1_min_trailing_silence = 2.4f;
      c.rule2_min_trailing_silence = 1.2f;
      c.rule3_min_utterance_length = 20;
      r->online = r->SherpaOnnxCreateOnlineRecognizer(&c);
      if (!r->online) { snprintf(error, capacity, "Cannot load Paraformer model"); goto fail; }
      r->stream = r->SherpaOnnxCreateOnlineStream(r->online);
      if (!r->stream) { snprintf(error, capacity, "Cannot create Paraformer stream"); goto fail; }
    } else {
      SherpaOnnxOfflineRecognizerConfig c = {};
      c.feat_config.sample_rate = 16000;
      c.feat_config.feature_dim = 80;
      c.model_config.whisper.encoder = encoder;
      c.model_config.whisper.decoder = decoder;
      c.model_config.whisper.language = language;
      c.model_config.whisper.task = "transcribe";
      c.model_config.tokens = tokens;
      c.model_config.num_threads = 2;
      c.model_config.provider = "cpu";
      c.decoding_method = "greedy_search";
      r->offline = r->SherpaOnnxCreateOfflineRecognizer(&c);
      if (!r->offline) { snprintf(error, capacity, "Cannot load Whisper Tiny model"); goto fail; }
    }
    return r;
  } CATCH_ERROR(error, capacity)
fail:
  if (!error[0]) snprintf(error, capacity, "Local runtime initialization failed");
  vp_local_close(r);
  return nullptr;
}
#define OPERATION(body) \
  try { body; } CATCH_ERROR(r->error, sizeof(r->error)) \
  return -1;
int vp_local_feed(Runtime *r, const float *samples, int32_t count) {
  OPERATION(r->SherpaOnnxOnlineStreamAcceptWaveform(r->stream, 16000, samples, count); return 0)
}
int vp_local_ready(Runtime *r) { OPERATION(return r->SherpaOnnxIsOnlineStreamReady(r->online, r->stream)) }
int vp_local_decode(Runtime *r) { OPERATION(r->SherpaOnnxDecodeOnlineStream(r->online, r->stream); return 0) }
int vp_local_endpoint(Runtime *r) { OPERATION(return r->SherpaOnnxOnlineStreamIsEndpoint(r->online, r->stream)) }
int vp_local_reset(Runtime *r) { OPERATION(r->SherpaOnnxOnlineStreamReset(r->online, r->stream); return 0) }
int vp_local_finish(Runtime *r) { OPERATION(r->SherpaOnnxOnlineStreamInputFinished(r->stream); return 0) }
char *vp_local_result(Runtime *r) {
  try {
    const char *json = r->SherpaOnnxGetOnlineStreamResultAsJson(r->online, r->stream);
    if (!json) return nullptr;
    char *copy = strdup(json);
    r->SherpaOnnxDestroyOnlineStreamResultJson(json);
    return copy;
  } CATCH_ERROR(r->error, sizeof(r->error))
  return nullptr;
}
char *vp_local_offline(Runtime *r, const float *samples, int32_t count) {
  const SherpaOnnxOfflineStream *stream = nullptr;
  char *copy = nullptr;
  try {
    stream = r->SherpaOnnxCreateOfflineStream(r->offline);
    if (!stream) return nullptr;
    r->SherpaOnnxAcceptWaveformOffline(stream, 16000, samples, count);
    r->SherpaOnnxDecodeOfflineStream(r->offline, stream);
    const char *json = r->SherpaOnnxGetOfflineStreamResultAsJson(stream);
    copy = json ? strdup(json) : nullptr;
    if (json) r->SherpaOnnxDestroyOfflineStreamResultJson(json);
  } CATCH_ERROR(r->error, sizeof(r->error))
  if (stream) r->SherpaOnnxDestroyOfflineStream(stream);
  return copy;
}
void vp_local_free(char *text) { free(text); }
}
