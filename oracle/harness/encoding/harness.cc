#include <cstdint>
#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <memory>
#include <string>
#include <vector>
#include <unicode/udata.h>
#include "third_party/blink/renderer/platform/wtf/allocator/partitions.h"
#include "third_party/blink/renderer/platform/wtf/text/text_encoding.h"
#include "third_party/blink/renderer/platform/wtf/text/text_encoding_registry.h"
#include "third_party/blink/renderer/platform/wtf/wtf.h"

using namespace blink;
extern "C" const unsigned char _binary_icudtl_dat_start[];

static uint32_t number(const std::vector<uint8_t>& in, size_t& pos) {
  if (in.size() - pos < 4) abort();
  uint32_t result = 0;
  for (int i = 0; i < 4; ++i) result = result * 256 + in[pos++];
  return result;
}
static void number(std::vector<uint8_t>& out, uint32_t value) {
  for (int i = 3; i >= 0; --i) out.push_back(value >> (8 * i));
}
static std::vector<uint8_t> blob(const std::vector<uint8_t>& in, size_t& pos) {
  auto size = number(in, pos);
  if (size > in.size() - pos) abort();
  std::vector<uint8_t> value(in.begin() + pos, in.begin() + pos + size);
  pos += size;
  return value;
}
static bool ascii_space(uint8_t c) {
  return c == 0x09 || c == 0x0a || c == 0x0c || c == 0x0d || c == 0x20;
}
static std::vector<uint8_t> record(const std::vector<uint8_t>& in) {
  if (in.empty()) abort();
  if (in[0] == 4) {
    size_t pos = 1;
    auto count = number(in, pos);
    if (!count || count > 1024) abort();
    std::vector<uint8_t> result;
    for (uint32_t i = 0; i < count; ++i) {
      auto item = blob(in, pos);
      if (item.empty() || item[0] == 4 || item[0] == 3) abort();
      auto response = record(item);
      number(result, response.size());
      result.insert(result.end(), response.begin(), response.end());
    }
    if (pos != in.size()) abort();
    return result;
  }
  if (in[0] == 3) {
    if (in.size() != 1) abort();
    std::vector<std::string> labels;
    for (const auto& label : TextEncodingAliasesForTesting()) {
      auto bytes = label.Utf8();
      labels.emplace_back(bytes.data(), bytes.length());
    }
    std::sort(labels.begin(), labels.end());
    std::vector<uint8_t> result;
    for (const auto& label : labels) {
      result.insert(result.end(), label.begin(), label.end());
      result.push_back('\n');
    }
    return result;
  }
  size_t pos = 1;
  auto label = blob(in, pos);
  size_t start = 0;
  size_t end = label.size();
  while (start < end && ascii_space(label[start])) ++start;
  while (end > start && ascii_space(label[end - 1])) --end;
  TextEncoding encoding{StringView(base::span<const LChar>(label).subspan(start, end - start))};
  if (in[0] == 0 || in[0] == 5) {
    if (pos != in.size()) abort();
    if (!encoding.IsValid()) return {};
    auto name = encoding.GetName().GetString().ToAsciiLower().Utf8();
    std::string canonical(name.data(), name.length());
    if (in[0] == 5 && canonical == "replacement") return {};
    if (canonical == "iso-8859-1" || canonical == "us-ascii") canonical = "windows-1252";
    return std::vector<uint8_t>(canonical.begin(), canonical.end());
  }
  if (!encoding.IsValid()) abort();
  auto codec = NewTextCodec(encoding);
  if (in[0] == 1) {
    if (in.size() - pos < 2) abort();
    uint8_t mode = in[pos++];
    uint8_t fatal = in[pos++];
    if (mode > 2 || fatal > 1) abort();
    auto count = number(in, pos);
    if (!count || count > 65536) abort();
    std::vector<std::vector<uint8_t>> chunks;
    for (uint32_t i = 0; i < count; ++i) chunks.push_back(blob(in, pos));
    if (pos != in.size()) abort();
    std::vector<uint8_t> out(1);
    std::vector<uint8_t> pending;
    bool decided = mode == 0;
    for (uint32_t i = 0; i < count; ++i) {
      auto chunk = chunks[i];
      bool error = false;
      if (!decided) {
        pending.insert(pending.end(), chunk.begin(), chunk.end());
        struct Bom { const char* name; std::vector<uint8_t> bytes; };
        const std::vector<Bom> boms = {{"UTF-8", {0xef, 0xbb, 0xbf}}, {"UTF-16LE", {0xff, 0xfe}}, {"UTF-16BE", {0xfe, 0xff}}};
        bool maybe = false;
        for (const auto& bom : boms) {
          TextEncoding bom_encoding{StringView(bom.name)};
          if (mode == 1 && !(encoding == bom_encoding)) continue;
          size_t n = std::min(pending.size(), bom.bytes.size());
          if (!std::equal(pending.begin(), pending.begin() + n, bom.bytes.begin())) continue;
          if (pending.size() < bom.bytes.size()) { maybe = true; continue; }
          if (mode == 2) codec = NewTextCodec(bom_encoding);
          pending.erase(pending.begin(), pending.begin() + bom.bytes.size());
          decided = true;
          break;
        }
        if (!decided && (!maybe || i + 1 == count)) decided = true;
        if (decided) chunk.swap(pending);
      }
      String text;
      if (decided) text = codec->Decode(chunk, i + 1 == count ? FlushBehavior::kDataEof : FlushBehavior::kDoNotFlush, fatal, error);
      if (fatal && error) text = String();
      if (!fatal) error = false;
      number(out, text.length());
      for (unsigned j = 0; j < text.length(); ++j) {
        out.push_back(text[j] >> 8);
        out.push_back(text[j]);
      }
      out.push_back(error);
      out[0] |= error;
      if (fatal && error) break;
    }
    return out;
  }
  if (in[0] == 2) {
    auto bytes = blob(in, pos);
    if (bytes.size() % 2 || pos != in.size()) abort();
    std::vector<UChar> text;
    for (size_t i = 0; i < bytes.size(); i += 2) text.push_back((bytes[i] << 8) | bytes[i + 1]);
    auto encoder = NewTextCodec(encoding.EncodingForFormSubmission());
    auto encoded = encoder->Encode(text, UnencodableHandling::kXmlCharRef);
    std::vector<uint8_t> out;
    out.insert(out.end(), encoded.begin(), encoded.end());
    return out;
  }
  abort();
}
int main() {
  UErrorCode error = U_ZERO_ERROR;
  udata_setCommonData(_binary_icudtl_dat_start, &error);
  if (U_FAILURE(error)) return 1;
  Partitions::Initialize();
  InitializeWtf();
  char header[4];
  if (fread(header, 1, 4, stdin) != 4 || std::string(header, 4) != "MGB1") return 1;
  fwrite(header, 1, 4, stdout);
  fflush(stdout);
  while (fread(header, 1, 4, stdin) == 4) {
    uint32_t size = 0;
    for (char c : header) size = size * 256 + static_cast<uint8_t>(c);
    if (size > 16 * 1024 * 1024) return 1;
    std::vector<uint8_t> in(size);
    if (fread(in.data(), 1, size, stdin) != size) return 1;
    auto out = record(in);
    std::vector<uint8_t> length;
    number(length, out.size());
    fwrite(length.data(), 1, 4, stdout);
    fwrite(out.data(), 1, out.size(), stdout);
    fflush(stdout);
  }
}
