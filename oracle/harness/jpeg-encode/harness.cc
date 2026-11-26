#include <cstdint>
#include <cstdio>
#include <cstring>
#include <vector>

#include "include/core/SkColorSpace.h"
#include "include/core/SkData.h"
#include "include/core/SkImageInfo.h"
#include "include/core/SkPixmap.h"
#include "include/core/SkStream.h"
#include "include/encode/SkJpegEncoder.h"

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

bool encode(const Bytes &input, Bytes &output) {
  size_t offset = 1;
  uint32_t width, height, quality;
  if (!take32(input, offset, width) || !take32(input, offset, height) ||
      !take32(input, offset, quality))
    return false;
  if (width == 0 || height == 0 || width > 100000 || height > 100000)
    return true;
  uint64_t samples = uint64_t(width) * height * 4;
  if (samples > (uint64_t(1) << 28) || input.size() - offset != samples)
    return true;

  auto info = SkImageInfo::Make(int(width), int(height), kBGRA_8888_SkColorType,
                                kUnpremul_SkAlphaType, SkColorSpace::MakeSRGB());
  size_t row_bytes = size_t(width) * 4;
  SkPixmap pixmap(info, input.data() + offset, row_bytes);

  SkJpegEncoder::Options options;
  options.fQuality = int(quality);
  options.fAlphaOption = SkJpegEncoder::AlphaOption::kBlendOnBlack;
  options.fDownsample = quality == 100 ? SkJpegEncoder::Downsample::k444
                                       : SkJpegEncoder::Downsample::k420;
  SkDynamicMemoryWStream stream;
  if (!SkJpegEncoder::Encode(&stream, pixmap, options)) return true;
  sk_sp<SkData> encoded = stream.detachAsData();
  output.assign(encoded->bytes(), encoded->bytes() + encoded->size());
  return true;
}

}  // namespace

int main() {
  uint8_t header[4];
  if (fread(header, 1, 4, stdin) != 4 || memcmp(header, "MGB1", 4) != 0) return 1;
  if (fwrite("MGB1", 1, 4, stdout) != 4 || fflush(stdout)) return 1;
  while (fread(header, 1, 4, stdin) == 4) {
    uint32_t size = (uint32_t(header[0]) << 24) | (uint32_t(header[1]) << 16) |
                    (uint32_t(header[2]) << 8) | header[3];
    Bytes input(size), output;
    if (fread(input.data(), 1, size, stdin) != size || input.empty()) return 1;
    if (input[0] != 1 || !encode(input, output)) return 1;
    Bytes header_bytes;
    put32(header_bytes, uint32_t(output.size()));
    if (fwrite(header_bytes.data(), 1, 4, stdout) != 4 ||
        fwrite(output.data(), 1, output.size(), stdout) != output.size() ||
        fflush(stdout))
      return 1;
  }
  return ferror(stdin) ? 1 : 0;
}
