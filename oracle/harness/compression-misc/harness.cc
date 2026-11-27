#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>

#include "snappy.h"

namespace {

constexpr size_t kMaxRequest = 4u << 20;

uint32_t get32(const unsigned char* p) {
  return (uint32_t(p[0]) << 24) | (uint32_t(p[1]) << 16) | (uint32_t(p[2]) << 8) | uint32_t(p[3]);
}

void put32(unsigned char* p, uint32_t value) {
  p[0] = value >> 24;
  p[1] = value >> 16;
  p[2] = value >> 8;
  p[3] = value;
}

bool read_exact(FILE* file, void* buffer, size_t size) {
  return size == 0 || fread(buffer, 1, size, file) == size;
}

struct Answer {
  unsigned char status;
  size_t length;
  char* bytes;
};

Answer fail() { return {1, 0, nullptr}; }

Answer record(const char* input, size_t size) {
  if (size < 1) return fail();
  const unsigned op = static_cast<unsigned char>(input[0]);
  if (op == 0) {
    if (size < 5) return fail();
    const uint32_t n = get32(reinterpret_cast<const unsigned char*>(input + 1));
    if (n > kMaxRequest || size != size_t(n) + 5) return fail();
    const size_t capacity = snappy::MaxCompressedLength(n);
    char* compressed = static_cast<char*>(malloc(capacity));
    if (!compressed) return fail();
    size_t produced = 0;
    snappy::RawCompress(input + 5, n, compressed, &produced);
    return {0, produced, compressed};
  }
  if (op == 1) {
    if (size < 9) return fail();
    const unsigned char* head = reinterpret_cast<const unsigned char*>(input);
    const uint32_t limit = get32(head + 1);
    const uint32_t n = get32(head + 5);
    if (n > kMaxRequest || size != size_t(n) + 9) return fail();
    size_t length = 0;
    if (!snappy::GetUncompressedLength(input + 9, n, &length) || length > limit) return fail();
    char* output = static_cast<char*>(malloc(length ? length : 1));
    if (!output) return fail();
    if (!snappy::RawUncompress(input + 9, n, output)) {
      free(output);
      return fail();
    }
    return {0, length, output};
  }
  return fail();
}

}  // namespace

int main() {
  unsigned char head[4];
  if (!read_exact(stdin, head, 4) || std::memcmp(head, "MGB1", 4)) return 1;
  if (fwrite("MGB1", 1, 4, stdout) != 4 || fflush(stdout)) return 1;
  while (read_exact(stdin, head, 4)) {
    const uint32_t size = get32(head);
    if (size > kMaxRequest) return 1;
    char* input = static_cast<char*>(malloc(size ? size : 1));
    if (!input || !read_exact(stdin, input, size)) {
      free(input);
      return 1;
    }
    const Answer answer = record(input, size);
    free(input);
    unsigned char status[1] = {answer.status};
    put32(head, uint32_t(5 + answer.length));
    const int wrote = fwrite(head, 1, 4, stdout) == 4 &&
                      fwrite(status, 1, 1, stdout) == 1;
    put32(head, uint32_t(answer.length));
    const int complete = wrote && fwrite(head, 1, 4, stdout) == 4 &&
                         (answer.length == 0 || fwrite(answer.bytes, 1, answer.length, stdout) == answer.length) &&
                         fflush(stdout) == 0;
    free(answer.bytes);
    if (!complete) return 1;
  }
  return ferror(stdin) ? 1 : 0;
}
