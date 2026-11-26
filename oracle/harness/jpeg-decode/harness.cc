#include <csetjmp>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <vector>

#include "base/command_line.h"
#include "base/task/single_thread_task_executor.h"
#include "third_party/blink/public/platform/platform.h"
#include "third_party/blink/renderer/platform/image-decoders/image_decoder.h"
#include "third_party/blink/renderer/platform/image-decoders/image_frame.h"
#include "third_party/blink/renderer/platform/image-decoders/jpeg/jpeg_image_decoder.h"
#include "third_party/blink/renderer/platform/image-decoders/rw_buffer.h"
#include "third_party/blink/renderer/platform/image-decoders/segment_reader.h"
#include "third_party/blink/renderer/platform/wtf/shared_buffer.h"
#include "third_party/skia/include/core/SkBitmap.h"
#include "third_party/skia/include/core/SkData.h"
#include "third_party/skia/include/private/SkJpegMetadataDecoder.h"

extern "C" {
#include "jpeglib.h"
}

using Bytes = std::vector<uint8_t>;

namespace {

constexpr size_t kMaxFrameBytes = 64u << 20;
constexpr size_t kSniffLength = 14;
constexpr uint32_t kNone = 0xffffffffu;

void put32(Bytes& output, uint32_t value) {
  output.push_back(value >> 24);
  output.push_back(value >> 16);
  output.push_back(value >> 8);
  output.push_back(value);
}

bool take32(const Bytes& data, size_t& offset, uint32_t& value) {
  if (data.size() - offset < 4) return false;
  value = (uint32_t(data[offset]) << 24) | (uint32_t(data[offset + 1]) << 16) |
          (uint32_t(data[offset + 2]) << 8) | data[offset + 3];
  offset += 4;
  return true;
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

struct HeaderError {
  jpeg_error_mgr pub;
  jmp_buf jump;
};

void HeaderErrorExit(j_common_ptr info) { longjmp(reinterpret_cast<HeaderError*>(info->err)->jump, 1); }

void HeaderMessage(j_common_ptr, int) {}

void PutICCProfile(const Bytes& data, Bytes& output) {
  jpeg_decompress_struct info = {};
  HeaderError error;
  info.err = jpeg_std_error(&error.pub);
  error.pub.error_exit = HeaderErrorExit;
  error.pub.emit_message = HeaderMessage;
  jpeg_create_decompress(&info);
  if (setjmp(error.jump)) {
    jpeg_destroy_decompress(&info);
    put32(output, kNone);
    return;
  }
  jpeg_mem_src(&info, data.data(), data.size());
  jpeg_save_markers(&info, JPEG_APP0 + 1, 0xFFFF);
  jpeg_save_markers(&info, JPEG_APP0 + 2, 0xFFFF);
  jpeg_save_markers(&info, JPEG_APP0 + 11, 0xFFFF);
  jpeg_read_header(&info, TRUE);
  std::vector<SkJpegMetadataDecoder::Segment> segments;
  for (auto* marker = info.marker_list; marker; marker = marker->next) {
    segments.emplace_back(marker->marker, SkData::MakeWithoutCopy(marker->data, marker->data_length));
  }
  auto profile = SkJpegMetadataDecoder::Make(std::move(segments))->getICCProfileData(/*copyData=*/true);
  jpeg_destroy_decompress(&info);
  if (!profile) {
    put32(output, kNone);
    return;
  }
  put32(output, profile->size());
  output.insert(output.end(), profile->bytes(), profile->bytes() + profile->size());
}

struct Image {
  scoped_refptr<blink::SharedBuffer> shared = blink::SharedBuffer::Create();
  std::unique_ptr<blink::ImageDecoder> header;
  std::unique_ptr<blink::RWBuffer> parked;
  std::unique_ptr<blink::ImageDecoder> frames;
  CappedAllocator allocator;

  void Park() {
    if (!parked) parked = std::make_unique<blink::RWBuffer>(shared->size());
    for (auto it = shared->GetIteratorAt(parked->size()); it != shared->cend(); ++it) {
      const size_t remaining = shared->size() - parked->size() - it->size();
      parked->Append(base::as_byte_span(*it), remaining);
    }
  }

  void Step(const Bytes& prefix, base::span<const uint8_t> chunk, bool all_received, Bytes& output) {
    shared->Append(chunk);
    if (!header) {
      if (!all_received && prefix.size() < kSniffLength) {
        put32(output, 0);
        return;
      }
      if (prefix.size() < 3 || prefix[0] != 0xFF || prefix[1] != 0xD8 || prefix[2] != 0xFF) {
        put32(output, 1);
        return;
      }
      header = std::make_unique<blink::JPEGImageDecoder>(
          blink::ImageDecoder::kAlphaPremultiplied, blink::ColorBehavior::kTag, cc::AuxImage::kDefault,
          blink::ImageDecoder::kNoDecodedImageByteLimit, gfx::Size());
    }
    put32(output, 2);
    header->SetData(shared, all_received);
    const bool available = header->IsSizeAvailable();
    put32(output, available);
    put32(output, header->Failed());
    if (available) {
      put32(output, header->Size().width());
      put32(output, header->Size().height());
      put32(output, static_cast<uint32_t>(header->Orientation()));
      put32(output, header->DensityCorrectedSize().width());
      put32(output, header->DensityCorrectedSize().height());
      PutICCProfile(prefix, output);
      if (!frames) {
        Park();
        frames = std::make_unique<blink::JPEGImageDecoder>(
            blink::ImageDecoder::kAlphaPremultiplied, blink::ColorBehavior::kIgnore, cc::AuxImage::kDefault,
            blink::ImageDecoder::kNoDecodedImageByteLimit, header->Size());
        frames->SetData(blink::SegmentReader::CreateFromROBuffer(parked->MakeROBufferSnapshot()), all_received);
      } else {
        Park();
        frames->SetData(blink::SegmentReader::CreateFromROBuffer(parked->MakeROBufferSnapshot()), all_received);
      }
    } else if (frames) {
      Park();
      frames->SetData(blink::SegmentReader::CreateFromROBuffer(parked->MakeROBufferSnapshot()), all_received);
    }
    put32(output, frames != nullptr);
    if (!frames) return;
    if (all_received) frames->FrameCount();
    frames->SetMemoryAllocator(&allocator);
    blink::ImageFrame* frame = frames->DecodeFrameBufferAtIndex(0);
    frames->SetMemoryAllocator(nullptr);
    frames->SetData(scoped_refptr<blink::SegmentReader>(nullptr), false);
    put32(output, frames->Failed());
    if (!frame) {
      put32(output, kNone);
      return;
    }
    put32(output, frame->GetStatus());
    put32(output, frame->HasAlpha());
    const SkBitmap& bitmap = frame->Bitmap();
    if (bitmap.isNull() || !bitmap.getPixels()) {
      put32(output, 0);
      put32(output, 0);
      put32(output, 0);
      return;
    }
    put32(output, bitmap.width());
    put32(output, bitmap.height());
    const size_t bytes = size_t(bitmap.width()) * bitmap.height() * 4;
    put32(output, bytes);
    for (int y = 0; y < bitmap.height(); ++y) {
      const uint8_t* row = static_cast<const uint8_t*>(bitmap.getAddr(0, y));
      output.insert(output.end(), row, row + size_t(bitmap.width()) * 4);
    }
  }
};

bool Decode(const Bytes& input, Bytes& output) {
  size_t offset = 1;
  uint32_t count;
  if (!take32(input, offset, count) || count == 0) return false;
  Image image;
  Bytes prefix;
  for (uint32_t index = 0; index < count; ++index) {
    uint32_t size;
    if (!take32(input, offset, size) || size > input.size() - offset) return false;
    base::span<const uint8_t> chunk(input.data() + offset, size);
    prefix.insert(prefix.end(), input.begin() + offset, input.begin() + offset + size);
    offset += size;
    image.Step(prefix, chunk, index + 1 == count, output);
  }
  return offset == input.size();
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
    if (input[0] != 0 || !Decode(input, output)) return 1;
    header[0] = output.size() >> 24; header[1] = output.size() >> 16;
    header[2] = output.size() >> 8; header[3] = output.size();
    if (fwrite(header, 1, 4, stdout) != 4 || fwrite(output.data(), 1, output.size(), stdout) != output.size() || fflush(stdout)) return 1;
  }
  return ferror(stdin) ? 1 : 0;
}
