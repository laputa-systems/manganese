#include <cstdint>
#include <cstdio>
#include <cstring>
#include <memory>
#include <vector>

#include "base/command_line.h"
#include "base/task/single_thread_task_executor.h"
#include "third_party/blink/public/platform/platform.h"
#include "third_party/blink/renderer/platform/image-decoders/image_decoder.h"
#include "third_party/blink/renderer/platform/image-decoders/image_frame.h"
#include "third_party/blink/renderer/platform/image-decoders/segment_reader.h"
#include "third_party/blink/renderer/platform/image-decoders/webp/webp_image_decoder.h"
#include "third_party/blink/renderer/platform/wtf/shared_buffer.h"
#include "third_party/skia/include/core/SkBitmap.h"
#include "third_party/skia/include/core/SkData.h"
#include "webp/decode.h"
#include "webp/demux.h"

using Bytes = std::vector<uint8_t>;

namespace {

constexpr size_t kMaxFrameBytes = 64u << 20;

void put32(Bytes& output, uint32_t value) {
  output.push_back(value >> 24);
  output.push_back(value >> 16);
  output.push_back(value >> 8);
  output.push_back(value);
}

class CappedAllocator final : public SkBitmap::Allocator {
 public:
  bool allocPixelRef(SkBitmap* bitmap) override {
    if (bitmap->computeByteSize() > kMaxFrameBytes) return false;
    return heap_.allocPixelRef(bitmap);
  }

 private:
  SkBitmap::HeapAllocator heap_;
};

bool Decode(const uint8_t* data, size_t size, Bytes& output) {
  WebPBitstreamFeatures features = {};
  if (size < 12 || memcmp(data, "RIFF", 4) || memcmp(data + 8, "WEBP", 4) ||
      WebPGetFeatures(data, size, &features) != VP8_STATUS_OK) {
    put32(output, 0);
    return true;
  }
  WebPData input = {data, size};
  WebPDemuxState state = WEBP_DEMUX_PARSING_HEADER;
  WebPDemuxer* demux = WebPDemuxPartial(&input, &state);
  uint32_t frame_count = demux ? WebPDemuxGetI(demux, WEBP_FF_FRAME_COUNT) : 1;
  WebPDemuxDelete(demux);

  auto shared = blink::SharedBuffer::Create();
  shared->Append(base::span<const uint8_t>(data, size));
  auto decoder = std::make_unique<blink::WEBPImageDecoder>(
      blink::ImageDecoder::kAlphaPremultiplied, blink::ColorBehavior::kIgnore,
      blink::ImageDecoder::kNoDecodedImageByteLimit);
  decoder->SetData(shared, true);
  if (decoder->Failed() || !decoder->IsSizeAvailable()) {
    put32(output, 0);
    return true;
  }
  const gfx::Size natural = decoder->Size();
  if (natural.IsEmpty() || static_cast<uint64_t>(natural.width()) * natural.height() * 4 > kMaxFrameBytes) {
    put32(output, 0);
    return true;
  }
  decoder->FrameCount();
  CappedAllocator allocator;
  decoder->SetMemoryAllocator(&allocator);
  blink::ImageFrame* frame = decoder->DecodeFrameBufferAtIndex(0);
  decoder->SetMemoryAllocator(nullptr);
  if (decoder->Failed() || !frame) {
    put32(output, 0);
    return true;
  }
  const SkBitmap& bitmap = frame->Bitmap();
  if (bitmap.isNull() || !bitmap.getPixels() || bitmap.width() != natural.width() || bitmap.height() != natural.height()) {
    put32(output, 0);
    return true;
  }
  put32(output, 1);
  put32(output, static_cast<uint32_t>(natural.width()));
  put32(output, static_cast<uint32_t>(natural.height()));
  put32(output, features.has_alpha ? 1u : 0u);
  put32(output, frame_count ? frame_count : 1u);
  put32(output, static_cast<uint32_t>(bitmap.width()));
  put32(output, static_cast<uint32_t>(bitmap.height()));
  const uint32_t bytes = static_cast<uint32_t>(bitmap.width()) * bitmap.height() * 4;
  put32(output, bytes);
  for (int y = 0; y < bitmap.height(); ++y) {
    const uint8_t* row = static_cast<const uint8_t*>(bitmap.getAddr(0, y));
    output.insert(output.end(), row, row + static_cast<size_t>(bitmap.width()) * 4);
  }
  return true;
}

}  // namespace

int main(int argc, char** argv) {
  base::SingleThreadTaskExecutor main_task_executor;
  base::CommandLine::Init(argc, argv);
  auto platform = std::make_unique<blink::Platform>();
  blink::Platform::CreateMainThreadAndInitialize(platform.get());

  uint8_t header[4];
  if (fread(header, 1, 4, stdin) != 4 || memcmp(header, "MGB1", 4) != 0) return 1;
  if (fwrite("MGB1", 1, 4, stdout) != 4 || fflush(stdout)) return 1;
  while (fread(header, 1, 4, stdin) == 4) {
    uint32_t size = (uint32_t(header[0]) << 24) | (uint32_t(header[1]) << 16) |
                    (uint32_t(header[2]) << 8) | header[3];
    Bytes input(size), output;
    if (fread(input.data(), 1, size, stdin) != size || input.empty()) return 1;
    if (input[0] != 0 || !Decode(input.data() + 1, input.size() - 1, output)) return 1;
    header[0] = output.size() >> 24; header[1] = output.size() >> 16;
    header[2] = output.size() >> 8; header[3] = output.size();
    if (fwrite(header, 1, 4, stdout) != 4 || fwrite(output.data(), 1, output.size(), stdout) != output.size() || fflush(stdout)) return 1;
  }
  return ferror(stdin) ? 1 : 0;
}
