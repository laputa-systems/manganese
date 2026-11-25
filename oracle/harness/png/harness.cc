#include <cstdint>
#include <cstdio>
#include <cstring>
#include <memory>
#include <vector>

#include "base/containers/span.h"
#include "third_party/blink/renderer/platform/graphics/color_behavior.h"
#include "third_party/blink/renderer/platform/image-decoders/png/png_image_decoder.h"
#include "third_party/blink/renderer/platform/wtf/shared_buffer.h"
#include "third_party/skia/include/core/SkBitmap.h"
#include "third_party/skia/include/core/SkImageInfo.h"
#include "third_party/skia/include/core/SkPixmap.h"
#include "third_party/skia/include/core/SkStream.h"
#include "third_party/skia/include/encode/SkPngRustEncoder.h"
#include "ui/gfx/codec/png_codec.h"

using Bytes = std::vector<uint8_t>;

static void put32(Bytes& output, uint32_t value) {
  output.push_back(value >> 24);
  output.push_back(value >> 16);
  output.push_back(value >> 8);
  output.push_back(value);
}

static bool take32(const Bytes& data, size_t& offset, uint32_t& value) {
  if (data.size() - offset < 4) return false;
  value = (uint32_t(data[offset]) << 24) | (uint32_t(data[offset + 1]) << 16) |
          (uint32_t(data[offset + 2]) << 8) | data[offset + 3];
  offset += 4;
  return true;
}

static bool decode(const Bytes& input, Bytes& output) {
  size_t offset = 1;
  uint32_t count;
  if (!take32(input, offset, count)) return false;
  auto decoder = std::make_unique<blink::PngImageDecoder>(
      blink::ImageDecoder::kAlphaPremultiplied, blink::ColorBehavior::kIgnore,
      blink::ImageDecoder::kNoDecodedImageByteLimit);
  auto data = blink::SharedBuffer::Create();
  for (uint32_t index = 0; index < count; ++index) {
    uint32_t size;
    if (!take32(input, offset, size) || size > input.size() - offset) return false;
    data->Append(base::span(reinterpret_cast<const char*>(input.data() + offset), size));
    offset += size;
    decoder->SetData(data.get(), index + 1 == count);
    bool available = decoder->IsSizeAvailable();
    put32(output, available);
    put32(output, decoder->Failed());
    put32(output, available ? decoder->Size().width() : 0);
    put32(output, available ? decoder->Size().height() : 0);
    auto frames = decoder->FrameCount();
    put32(output, frames);
    put32(output, decoder->RepetitionCount());
    for (size_t frame_index = 0; frame_index < frames; ++frame_index) {
      auto* frame = decoder->DecodeFrameBufferAtIndex(frame_index);
      put32(output, frame != nullptr);
      if (!frame) continue;
      put32(output, frame->GetStatus());
      put32(output, frame->HasAlpha());
      put32(output, frame->Duration().InMilliseconds());
      put32(output, frame->GetDisposalMethod());
      put32(output, frame->GetAlphaBlendSource());
      auto rect = frame->OriginalFrameRect();
      put32(output, rect.x()); put32(output, rect.y());
      put32(output, rect.width()); put32(output, rect.height());
      const auto& bitmap = frame->Bitmap();
      size_t length = bitmap.getPixels() ? bitmap.computeByteSize() : 0;
      put32(output, length);
      const auto* pixels = static_cast<const uint8_t*>(bitmap.getPixels());
      if (length) output.insert(output.end(), pixels, pixels + length);
    }
    put32(output, decoder->Failed());
  }
  return offset == input.size();
}

static bool encode(const Bytes& input, Bytes& output) {
  size_t offset = 1;
  uint32_t width, height, mode;
  if (!take32(input, offset, width) || !take32(input, offset, height) ||
      !take32(input, offset, mode) || mode > 5 || width == 0 || height == 0 ||
      (input.size() - offset) % 4 || uint64_t(width) * height != (input.size() - offset) / 4) return false;
  auto alpha = mode >= 3 ? kOpaque_SkAlphaType : kPremul_SkAlphaType;
  mode %= 3;
  auto info = SkImageInfo::Make(width, height, kN32_SkColorType, alpha);
  SkPixmap pixmap(info, input.data() + offset, size_t(width) * 4);
  if (mode == 0) {
    SkDynamicMemoryWStream stream;
    SkPngRustEncoder::Options options;
    options.fCompressionLevel = SkPngRustEncoder::CompressionLevel::kLow;
    if (!SkPngRustEncoder::Encode(&stream, pixmap, options)) return false;
    auto encoded = stream.detachAsData();
    output.insert(output.end(), encoded->bytes(), encoded->bytes() + encoded->size());
    return true;
  }
  SkBitmap bitmap;
  if (!bitmap.installPixels(info, const_cast<uint8_t*>(input.data() + offset), size_t(width) * 4)) return false;
  auto encoded = mode == 1 ? gfx::PNGCodec::EncodeBGRASkBitmap(bitmap, false)
                           : gfx::PNGCodec::FastEncodeBGRASkBitmap(bitmap, false);
  if (!encoded) return false;
  output = std::move(*encoded);
  return true;
}

int main() {
  uint8_t header[4];
  if (fread(header, 1, 4, stdin) != 4 || memcmp(header, "MGB1", 4) != 0) return 1;
  if (fwrite("MGB1", 1, 4, stdout) != 4 || fflush(stdout)) return 1;
  while (fread(header, 1, 4, stdin) == 4) {
    uint32_t size = (uint32_t(header[0]) << 24) | (uint32_t(header[1]) << 16) |
                    (uint32_t(header[2]) << 8) | header[3];
    Bytes input(size), output;
    if (fread(input.data(), 1, size, stdin) != size || input.empty()) return 1;
    if (!(input[0] == 0 ? decode(input, output) : input[0] == 1 && encode(input, output))) return 1;
    header[0] = output.size() >> 24; header[1] = output.size() >> 16;
    header[2] = output.size() >> 8; header[3] = output.size();
    if (fwrite(header, 1, 4, stdout) != 4 || fwrite(output.data(), 1, output.size(), stdout) != output.size() || fflush(stdout)) return 1;
  }
  return ferror(stdin) ? 1 : 0;
}
