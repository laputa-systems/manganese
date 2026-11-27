#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <optional>
#include <set>
#include <vector>

#include "include/codec/SkCodec.h"
#include "include/codec/SkCodecAnimation.h"
#include "include/codec/SkGifDecoder.h"
#include "include/core/SkAlphaType.h"
#include "include/core/SkColorType.h"
#include "include/core/SkImageInfo.h"
#include "include/core/SkStream.h"
#include "include/core/SkTypes.h"

using Bytes = std::vector<uint8_t>;

namespace {

constexpr int kLoopOnce = 0;
constexpr int kLoopInfinite = -1;
constexpr int kNone = -2;
constexpr uint64_t kMaxFrameBytes = 64u << 20;
constexpr uint32_t kMaxSide = 1000000;

enum Status : uint32_t { kEmpty, kInitialized, kPartial, kComplete };

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

class PrefixStream : public SkStream {
 public:
  PrefixStream(const uint8_t* data, size_t size) : data_(data), size_(size) {}

  size_t read(void* buffer, size_t size) override {
    if (position_ > size_) return 0;
    size = std::min(size, size_ - position_);
    if (buffer) memcpy(buffer, data_ + position_, size);
    position_ += size;
    return size;
  }
  size_t peek(void* buffer, size_t size) const override {
    if (position_ > size_) return 0;
    size = std::min(size, size_ - position_);
    memcpy(buffer, data_ + position_, size);
    return size;
  }
  bool isAtEnd() const override { return position_ >= size_; }
  bool rewind() override { position_ = 0; return true; }
  bool hasPosition() const override { return true; }
  size_t getPosition() const override { return position_; }
  bool seek(size_t position) override { position_ = position; return true; }
  bool move(long offset) override { position_ += offset; return true; }
  bool hasLength() const override { return true; }
  size_t getLength() const override { return size_; }

 private:
  const uint8_t* data_;
  size_t size_;
  size_t position_ = 0;
};

struct Frame {
  uint32_t status = kEmpty;
  bool has_alpha = true;
  int duration = 0;
  int required = -1;
  int disposal = 0;
  int blend = 0;
  SkIRect rect = SkIRect::MakeEmpty();
  Bytes pixels;
  bool allocated = false;
};

struct Decoder {
  std::unique_ptr<SkCodec> codec;
  bool failed = false;
  bool size_available = false;
  bool all_received = false;
  int width = 0;
  int height = 0;
  std::vector<Frame> frames;
  std::set<size_t> failed_frames;
  std::optional<size_t> already_started;
  int prior_frame = SkCodec::kNoFrame;
  int repetition_count = kLoopOnce;

  void SetFailed() {
    codec.reset();
    failed = true;
  }

  size_t DecodeFrameCount() { return codec ? size_t(codec->getFrameCount()) : frames.size(); }

  void SetFailedFrame(size_t index) {
    failed_frames.insert(index);
    if (failed_frames.size() == DecodeFrameCount()) SetFailed();
  }

  bool FrameIsReceived(size_t index) {
    if (all_received) return true;
    SkCodec::FrameInfo info;
    return codec && codec->getFrameInfo(int(index), &info) && info.fFullyReceived;
  }

  size_t FrameCount() {
    size_t old_size = frames.size();
    size_t new_size = DecodeFrameCount();
    if (old_size != new_size) {
      frames.resize(new_size);
      for (size_t i = old_size; i < new_size; ++i) InitializeNewFrame(i);
    }
    return new_size;
  }

  void InitializeNewFrame(size_t index) {
    SkCodec::FrameInfo info;
    if (!codec->getFrameInfo(int(index), &info)) abort();
    Frame& frame = frames[index];
    frame.duration = info.fDuration;
    frame.required = info.fRequiredFrame;
    frame.rect = info.fFrameRect;
    switch (info.fDisposalMethod) {
      case SkCodecAnimation::DisposalMethod::kKeep: frame.disposal = 0; break;
      case SkCodecAnimation::DisposalMethod::kRestoreBGColor: frame.disposal = 1; break;
      case SkCodecAnimation::DisposalMethod::kRestorePrevious: frame.disposal = 2; break;
      default: frame.disposal = 3; break;
    }
    frame.blend = info.fBlend == SkCodecAnimation::Blend::kSrc ? 1 : 0;
  }

  bool CanReuse(size_t index) const { return frames[index].disposal != 2; }

  size_t ViableReference(size_t dependent) const {
    size_t required = size_t(frames[dependent].required);
    if (required > dependent) return SIZE_MAX;
    for (size_t i = dependent; i != required;) {
      --i;
      const Frame& frame = frames[i];
      if (frame.disposal == 2) continue;
      if (frame.status == kComplete) return i;
    }
    return SIZE_MAX;
  }

  void Decode(size_t index) {
    struct Item { size_t index; size_t previous; };
    std::vector<Item> stack{{index, SIZE_MAX}};
    while (!stack.empty()) {
      Item item = stack.back();
      stack.pop_back();
      size_t current = item.index;
      size_t previous = item.previous;
      if (!codec || failed_frames.count(current) || current >= frames.size()) continue;
      Frame& frame = frames[current];
      if (frame.status == kComplete) continue;
      if (frame.status == kEmpty) {
        already_started.reset();
        if (frame.required < 0) {
          uint64_t bytes = uint64_t(width) * height * 4;
          if (bytes > kMaxFrameBytes) {
            SetFailedFrame(current);
            continue;
          }
          frame.pixels.assign(bytes, 0);
          frame.allocated = true;
          frame.status = kInitialized;
          prior_frame = SkCodec::kNoFrame;
        } else {
          if (previous == SIZE_MAX) {
            previous = ViableReference(current);
            if (previous == SIZE_MAX) {
              stack.push_back({current, size_t(frame.required)});
              stack.push_back({size_t(frame.required), SIZE_MAX});
              continue;
            }
          }
          if (failed_frames.count(previous) || previous >= frames.size()) {
            SetFailedFrame(current);
            continue;
          }
          frame.pixels = frames[previous].pixels;
          frame.allocated = true;
          frame.status = kInitialized;
          prior_frame = int(previous);
        }
      }
      bool started = already_started && *already_started == current;
      if (!started) {
        SkCodec::FrameInfo info;
        if (!codec->getFrameInfo(int(current), &info)) abort();
        SkAlphaType alpha = info.fAlphaType != kOpaque_SkAlphaType ? kPremul_SkAlphaType : kOpaque_SkAlphaType;
        SkImageInfo image_info = codec->getInfo()
                                     .makeColorType(kN32_SkColorType)
                                     .makeAlphaType(alpha)
                                     .makeColorSpace(nullptr);
        SkCodec::Options options;
        options.fFrameIndex = int(current);
        options.fPriorFrame = prior_frame;
        options.fZeroInitialized = SkCodec::kNo_ZeroInitialized;
        already_started.reset();
        SkCodec::Result result = codec->startIncrementalDecode(image_info, frame.pixels.data(), size_t(width) * 4, &options);
        if (result == SkCodec::kIncompleteInput) continue;
        if (result != SkCodec::kSuccess) {
          SetFailedFrame(current);
          continue;
        }
        frame.status = kPartial;
        already_started = current;
      }
      SkCodec::Result result = codec->incrementalDecode();
      if (result == SkCodec::kSuccess) {
        already_started.reset();
        SkCodec::FrameInfo info;
        if (!codec->getFrameInfo(int(current), &info)) abort();
        frame.has_alpha = info.fAlphaType != kOpaque_SkAlphaType;
        frame.status = kComplete;
      } else if (result == SkCodec::kIncompleteInput) {
        if (FrameIsReceived(current)) SetFailedFrame(current);
      } else {
        already_started.reset();
        SetFailedFrame(current);
      }
    }
  }

  void DecodeFrame(size_t index) {
    if (index >= FrameCount()) return;
    if (frames[index].status != kComplete) Decode(index);
  }

  int RepetitionCount() {
    if (!codec) return repetition_count;
    int count = codec->getRepetitionCount();
    if (count == 0) {
      switch (codec->isAnimated()) {
        case SkCodec::IsAnimated::kYes: repetition_count = kLoopOnce; return repetition_count;
        case SkCodec::IsAnimated::kNo: repetition_count = kNone; return repetition_count;
        case SkCodec::IsAnimated::kUnknown: break;
      }
      repetition_count = all_received && codec->getFrameCount() == 1 ? kNone : kLoopOnce;
    } else if (count == SkCodec::kRepetitionCountInfinite) {
      repetition_count = kLoopInfinite;
    } else {
      repetition_count = count;
    }
    return repetition_count;
  }
};

void Step(const Bytes& prefix, bool all_received, Bytes& output) {
  Decoder decoder;
  decoder.all_received = all_received;
  auto stream = std::make_unique<PrefixStream>(prefix.data(), prefix.size());
  SkCodec::Result result;
  decoder.codec = SkGifDecoder::Decode(std::move(stream), &result);
  switch (result) {
    case SkCodec::kSuccess: {
      SkImageInfo info = decoder.codec->getInfo();
      uint32_t width = uint32_t(info.width());
      uint32_t height = uint32_t(info.height());
      uint64_t total = uint64_t(width) * height * 4;
      if (width > kMaxSide || height > kMaxSide || total > 0x7fffffffu) {
        decoder.SetFailed();
      } else {
        decoder.width = int(width);
        decoder.height = int(height);
        decoder.size_available = true;
      }
      break;
    }
    case SkCodec::kIncompleteInput:
      if (all_received) decoder.SetFailed();
      break;
    default:
      decoder.SetFailed();
      break;
  }
  if (decoder.size_available && !decoder.failed) {
    for (size_t i = 0; i < decoder.FrameCount(); ++i) decoder.DecodeFrame(i);
  }
  int repetition = decoder.size_available ? decoder.RepetitionCount() : decoder.repetition_count;
  bool available = decoder.size_available && !decoder.failed;
  put32(output, available);
  put32(output, decoder.failed);
  put32(output, available ? decoder.width : 0);
  put32(output, available ? decoder.height : 0);
  put32(output, decoder.frames.size());
  put32(output, uint32_t(repetition));
  for (const Frame& frame : decoder.frames) {
    put32(output, frame.status);
    put32(output, frame.has_alpha);
    put32(output, uint32_t(frame.duration));
    put32(output, uint32_t(frame.required));
    put32(output, uint32_t(frame.disposal));
    put32(output, uint32_t(frame.blend));
    put32(output, uint32_t(frame.rect.x()));
    put32(output, uint32_t(frame.rect.y()));
    put32(output, uint32_t(frame.rect.width()));
    put32(output, uint32_t(frame.rect.height()));
    put32(output, frame.allocated ? frame.pixels.size() : 0);
    if (frame.allocated) output.insert(output.end(), frame.pixels.begin(), frame.pixels.end());
  }
}

bool decode(const Bytes& input, Bytes& output) {
  size_t offset = 1;
  uint32_t count;
  if (!take32(input, offset, count)) return false;
  Bytes data;
  for (uint32_t index = 0; index < count; ++index) {
    uint32_t size;
    if (!take32(input, offset, size) || size > input.size() - offset) return false;
    data.insert(data.end(), input.begin() + offset, input.begin() + offset + size);
    offset += size;
    Step(data, index + 1 == count, output);
  }
  return offset == input.size();
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
    if (input[0] != 0 || !decode(input, output)) return 1;
    header[0] = output.size() >> 24; header[1] = output.size() >> 16;
    header[2] = output.size() >> 8; header[3] = output.size();
    if (fwrite(header, 1, 4, stdout) != 4 || fwrite(output.data(), 1, output.size(), stdout) != output.size() || fflush(stdout)) return 1;
  }
  return ferror(stdin) ? 1 : 0;
}
