#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <memory>
#include <string>
#include <v8.h>
#include <libplatform/libplatform.h>
#include "engine.h"
#include "snapshot.h"

static uint32_t read_u32(const unsigned char* p) {
  return (uint32_t(p[0]) << 24) | (uint32_t(p[1]) << 16) | (uint32_t(p[2]) << 8) | p[3];
}

static void write_u32(unsigned char* p, uint32_t v) {
  p[0] = v >> 24; p[1] = v >> 16; p[2] = v >> 8; p[3] = v;
}

static std::string snapshot_path() {
  const char* scratch = getenv("GOLDENS_SCRATCH");
  if (scratch == nullptr || *scratch == 0) scratch = ".work/tmp";
  if (mkdir(scratch, 0700) != 0 && errno != EEXIST) return std::string();
  return std::string(scratch) + "/regex-snapshot.bin";
}

#define FAIL() do { fprintf(stderr, "regex harness fail at line %d\n", __LINE__); status = 1; goto done; } while (0)

int main() {
  std::string snapshot = snapshot_path();
  FILE* blob = fopen(snapshot.c_str(), "wb");
  if (blob == nullptr) return 2;
  if (fwrite(kSnapshotBlob, 1, kSnapshotBlobSize, blob) != kSnapshotBlobSize) return 2;
  fclose(blob);
  v8::V8::InitializeExternalStartupDataFromFile(snapshot.c_str());
  std::unique_ptr<v8::Platform> platform = v8::platform::NewDefaultPlatform();
  v8::V8::InitializePlatform(platform.get());
  v8::V8::Initialize();
  v8::Isolate::CreateParams params;
  params.array_buffer_allocator = v8::ArrayBuffer::Allocator::NewDefaultAllocator();
  v8::Isolate* isolate = v8::Isolate::New(params);
  int status = 0;
  {
    v8::Isolate::Scope isolate_scope(isolate);
    v8::HandleScope handle_scope(isolate);
    v8::Local<v8::Context> context = v8::Context::New(isolate);
    v8::Context::Scope context_scope(context);
    v8::Local<v8::String> source = v8::String::NewFromUtf8(isolate, kEngineSource, v8::NewStringType::kNormal).ToLocalChecked();
    v8::Local<v8::Script> script = v8::Script::Compile(context, source).ToLocalChecked();
    script->Run(context).ToLocalChecked();
    v8::Local<v8::Value> engine_value = context->Global()->Get(context, v8::String::NewFromUtf8Literal(isolate, "__regexEngine")).ToLocalChecked();
    if (!engine_value->IsFunction()) return 1;
    v8::Local<v8::Function> engine = engine_value.As<v8::Function>();

    char magic[4];
    if (fread(magic, 1, 4, stdin) != 4 || memcmp(magic, "MGB1", 4)) return 1;
    if (fwrite("MGB1", 1, 4, stdout) != 4) return 1;
    fflush(stdout);
    unsigned char header[4];
    while (fread(header, 1, 4, stdin) == 4) {
      uint32_t length = read_u32(header);
      if (length < 11 || length > 65536) FAIL();
      unsigned char* input = static_cast<unsigned char*>(malloc(length));
      if (!input || fread(input, 1, length, stdin) != length) FAIL();
      if (input[0] != 1) FAIL();
      uint32_t offset = 1;
      if (offset + 2 > length) FAIL();
      uint32_t pattern_len = uint32_t(input[offset]) << 8 | input[offset + 1]; offset += 2;
      if (offset + pattern_len + 2 > length) FAIL();
      const char* pattern = reinterpret_cast<const char*>(input + offset); offset += pattern_len;
      uint32_t flags_len = uint32_t(input[offset]) << 8 | input[offset + 1]; offset += 2;
      if (offset + flags_len + 2 > length) FAIL();
      const char* flags = reinterpret_cast<const char*>(input + offset); offset += flags_len;
      uint32_t subject_len = uint32_t(input[offset]) << 8 | input[offset + 1]; offset += 2;
      if (offset + subject_len + 4 > length) FAIL();
      const char* subject = reinterpret_cast<const char*>(input + offset); offset += subject_len;
      uint32_t iterations = uint32_t(input[offset]) << 8 | input[offset + 1]; offset += 2;
      uint32_t start = uint32_t(input[offset]) << 8 | input[offset + 1]; offset += 2;
      if (offset != length || iterations < 1 || iterations > 8 || start > 65535) FAIL();

      v8::Local<v8::Value> call_args[5] = {
        v8::String::NewFromUtf8(isolate, pattern, v8::NewStringType::kNormal, int(pattern_len)).ToLocalChecked(),
        v8::String::NewFromUtf8(isolate, flags, v8::NewStringType::kNormal, int(flags_len)).ToLocalChecked(),
        v8::String::NewFromUtf8(isolate, subject, v8::NewStringType::kNormal, int(subject_len)).ToLocalChecked(),
        v8::Integer::NewFromUnsigned(isolate, iterations),
        v8::Integer::NewFromUnsigned(isolate, start)
      };
      v8::TryCatch caught(isolate);
      v8::Local<v8::Value> result;
      if (!engine->Call(context, v8::Null(isolate), 5, call_args).ToLocal(&result)) {
        v8::String::Utf8Value message(isolate, caught.Exception());
        fprintf(stderr, "regex harness engine exception: %s\n", !caught.Exception().IsEmpty() ? *message : "unknown");
        FAIL();
      }
      v8::String::Utf8Value text(isolate, result);
      uint32_t out_len = uint32_t(text.length());
      write_u32(header, out_len);
      if (fwrite(header, 1, 4, stdout) != 4 || fwrite(*text, 1, out_len, stdout) != out_len) FAIL();
      fflush(stdout);
      free(input);
    }
  done:
    ;
  }
  isolate->Dispose();
  v8::V8::Dispose();
  v8::V8::DisposePlatform();
  delete params.array_buffer_allocator;
  return status;
}
