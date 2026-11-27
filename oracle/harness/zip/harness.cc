#include <clocale>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <string_view>
#include <vector>

#include "crypto/sha2.h"
#include "third_party/zlib/google/zip_reader.h"

using Bytes = std::vector<uint8_t>;

namespace {

bool take32(const Bytes &data, size_t &offset, uint32_t &value) {
  if (data.size() - offset < 4) return false;
  value = (uint32_t(data[offset]) << 24) | (uint32_t(data[offset + 1]) << 16) |
          (uint32_t(data[offset + 2]) << 8) | data[offset + 3];
  offset += 4;
  return true;
}

void put32(Bytes &output, uint32_t value) {
  output.push_back(value >> 24);
  output.push_back(value >> 16);
  output.push_back(value >> 8);
  output.push_back(value);
}

std::string hex(std::string_view bytes) {
  static const char digits[] = "0123456789abcdef";
  std::string result;
  result.reserve(bytes.size() * 2);
  for (unsigned char c : bytes) {
    result.push_back(digits[c >> 4]);
    result.push_back(digits[c & 15]);
  }
  return result;
}

void field(std::string &out, const char *name, uint64_t value) {
  out += name;
  out += ' ';
  out += std::to_string(value);
  out += '\n';
}

bool read_archive(const Bytes &input, std::string &out) {
  if (input.empty() || input[0] != 1) return false;
  std::string data(reinterpret_cast<const char *>(input.data() + 1),
                   input.size() - 1);
  zip::ZipReader reader;
  if (!reader.OpenFromString(data)) {
    out = "open 0\n";
    return true;
  }
  out = "open 1\n";
  uint64_t index = 0;
  while (const zip::ZipReader::Entry *entry = reader.Next()) {
    field(out, "entry", index++);
    out += "path " + hex(entry->path.value()) + "\n";
    out += "raw " + hex(entry->path_in_original_encoding) + "\n";
    out += "physical " + hex(entry->physical_path.value()) + "\n";
    field(out, "size", uint64_t(entry->original_size));
    field(out, "time", uint64_t(entry->last_modified.ToInternalValue()));
    field(out, "dir", entry->is_directory ? 1 : 0);
    field(out, "unsafe", entry->is_unsafe ? 1 : 0);
    field(out, "encrypted", entry->is_encrypted ? 1 : 0);
    field(out, "aes", entry->uses_aes_encryption ? 1 : 0);
    field(out, "mode", uint32_t(entry->posix_mode));
    field(out, "symlink", entry->is_symbolic_link ? 1 : 0);
    if (entry->is_directory || entry->is_encrypted) {
      out += "payload none 0 " + std::string(64, '0') + "\n";
      continue;
    }
    std::string payload;
    const bool complete =
        reader.ExtractCurrentEntryToString(1u << 20, &payload);
    out += complete ? "payload ok " : "payload fail ";
    out += std::to_string(payload.size());
    out += ' ';
    out += hex(crypto::SHA256HashString(payload));
    out += '\n';
  }
  field(out, "ok", reader.ok() ? 1 : 0);
  return true;
}

}  // namespace

int main() {
  setlocale(LC_ALL, "C.UTF-8");
  uint8_t header[4];
  if (fread(header, 1, 4, stdin) != 4 || memcmp(header, "MGB1", 4) != 0)
    return 1;
  if (fwrite("MGB1", 1, 4, stdout) != 4 || fflush(stdout)) return 1;
  while (fread(header, 1, 4, stdin) == 4) {
    uint32_t size = (uint32_t(header[0]) << 24) | (uint32_t(header[1]) << 16) |
                    (uint32_t(header[2]) << 8) | header[3];
    Bytes input(size), output;
    if (fread(input.data(), 1, size, stdin) != size || input.empty()) return 1;
    std::string result;
    if (!read_archive(input, result)) return 1;
    output.assign(result.begin(), result.end());
    Bytes header_bytes;
    put32(header_bytes, uint32_t(output.size()));
    if (fwrite(header_bytes.data(), 1, 4, stdout) != 4 ||
        fwrite(output.data(), 1, output.size(), stdout) != output.size() ||
        fflush(stdout))
      return 1;
  }
  return ferror(stdin) ? 1 : 0;
}
