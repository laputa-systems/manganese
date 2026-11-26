#include <cstdint>
#include <cstring>
#include <iostream>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "base/command_line.h"
#include "base/feature_list.h"
#include "base/functional/bind.h"
#include "base/memory_coordinator/dummy_memory_consumer_registry.h"
#include "base/memory_coordinator/memory_consumer_registry.h"
#include "base/run_loop.h"
#include "base/test/task_environment.h"
#include "base/test/test_timeouts.h"
#include "base/time/time.h"
#include "media/base/audio_decoder_config.h"
#include "media/base/decoder_buffer.h"
#include "media/base/demuxer.h"
#include "media/base/demuxer_stream.h"
#include "media/base/media_tracks.h"
#include "media/base/media_util.h"
#include "media/base/ranges.h"
#include "media/base/timestamp_constants.h"
#include "media/base/video_decoder_config.h"
#include "media/filters/ffmpeg_demuxer.h"
#include "media/ffmpeg/ffmpeg_common.h"
#include "media/filters/memory_data_source.h"
#include "third_party/boringssl/src/include/openssl/sha.h"

namespace {

const std::set<std::string_view> kScopedDemuxers = {"wav"};

std::string Time(base::TimeDelta value) {
  if (value == media::kNoTimestamp) return "none";
  if (value == media::kInfiniteDuration) return "inf";
  return std::to_string(value.InMicroseconds());
}

std::string Hex(const uint8_t* data, size_t size) {
  static const char kDigits[] = "0123456789abcdef";
  std::string out;
  for (size_t i = 0; i < size; ++i) {
    out += kDigits[data[i] >> 4];
    out += kDigits[data[i] & 15];
  }
  return out;
}

struct ProbeInput {
  const std::string* data;
  size_t position = 0;
};

int ProbeRead(void* opaque, uint8_t* buffer, int size) {
  auto* input = static_cast<ProbeInput*>(opaque);
  size_t left = input->data->size() - input->position;
  if (left == 0) return AVERROR_EOF;
  size_t count = std::min(left, static_cast<size_t>(size));
  std::memcpy(buffer, input->data->data() + input->position, count);
  input->position += count;
  return static_cast<int>(count);
}

int64_t ProbeSeek(void* opaque, int64_t offset, int whence) {
  auto* input = static_cast<ProbeInput*>(opaque);
  int64_t size = static_cast<int64_t>(input->data->size());
  int64_t target;
  switch (whence) {
    case SEEK_SET: target = offset; break;
    case SEEK_CUR: target = static_cast<int64_t>(input->position) + offset; break;
    case SEEK_END: target = size + offset; break;
    case AVSEEK_SIZE: return size;
    default: return AVERROR(EIO);
  }
  if (target < 0 || target > size) return AVERROR(EIO);
  input->position = static_cast<size_t>(target);
  return target;
}

std::string ProbedDemuxer(const std::string& data) {
  ProbeInput input{&data};
  constexpr int kBufferSize = 32 * 1024;
  AVIOContext* pb = avio_alloc_context(static_cast<unsigned char*>(av_malloc(kBufferSize)), kBufferSize, 0, &input,
                                       &ProbeRead, nullptr, &ProbeSeek);
  pb->seekable = AVIO_SEEKABLE_NORMAL;
  const AVInputFormat* format = nullptr;
  av_probe_input_buffer2(pb, &format, "", nullptr, 0, 0);
  av_freep(&pb->buffer);
  avio_context_free(&pb);
  return format ? format->name : "none";
}

class Host : public media::DemuxerHost {
 public:
  void OnBufferedTimeRangesChanged(const media::Ranges<base::TimeDelta>& ranges) override { buffered = ranges; }
  void SetDuration(base::TimeDelta duration) override { durations.push_back(duration); }
  void OnDemuxerError(media::PipelineStatus error) override { errors.push_back(static_cast<int>(error.code())); }

  media::Ranges<base::TimeDelta> buffered;
  std::vector<base::TimeDelta> durations;
  std::vector<int> errors;
};

std::string AudioConfig(const media::AudioDecoderConfig& config) {
  const std::vector<uint8_t>& extra = config.extra_data();
  return "codec=" + std::to_string(static_cast<int>(config.codec())) +
         " profile=" + std::to_string(static_cast<int>(config.profile())) +
         " format=" + std::to_string(static_cast<int>(config.sample_format())) +
         " layout=" + std::to_string(static_cast<int>(config.channel_layout())) +
         " channels=" + std::to_string(config.channels()) +
         " rate=" + std::to_string(config.samples_per_second()) +
         " preroll=" + std::to_string(config.seek_preroll().InMicroseconds()) +
         " delay=" + std::to_string(config.codec_delay()) +
         " encryption=" + std::to_string(static_cast<int>(config.encryption_scheme())) +
         " discard_delay=" + std::to_string(config.should_discard_decoder_delay() ? 1 : 0) +
         " extra=" + Hex(extra.data(), extra.size());
}

std::string Packet(const media::DecoderBuffer& buffer) {
  std::vector<uint8_t> bytes(buffer.begin(), buffer.end());
  uint8_t digest[SHA256_DIGEST_LENGTH];
  SHA256(bytes.data(), bytes.size(), digest);
  std::string discard = "none";
  if (auto padding = buffer.discard_padding()) discard = Time(padding->first) + "," + Time(padding->second);
  return Time(buffer.timestamp()) + " " + Time(buffer.duration()) + " " + (buffer.is_key_frame() ? "1" : "0") + " " +
         discard + " " + std::to_string(bytes.size()) + " " + Hex(digest, sizeof(digest)) + "\n";
}

std::string Execute(base::test::TaskEnvironment& environment, const std::string& data) {
  std::string probed = ProbedDemuxer(data);
  std::string output = "probe " + probed + "\n";
  if (probed != "none" && !kScopedDemuxers.contains(probed)) return output;

  media::MemoryDataSource source(base::as_byte_span(data));
  media::NullMediaLog log;
  Host host;
  auto demuxer = std::make_unique<media::FFmpegDemuxer>(
      base::SequencedTaskRunner::GetCurrentDefault(), &source,
      base::BindRepeating([](media::EmeInitDataType, const std::vector<uint8_t>&) {}),
      base::BindRepeating([](std::unique_ptr<media::MediaTracks>) {}), &log, false);
  int status = -1;
  {
    base::RunLoop loop;
    demuxer->Initialize(&host, base::BindOnce(
                                   [](int* status, base::OnceClosure done, media::PipelineStatus result) {
                                     *status = static_cast<int>(result.code());
                                     std::move(done).Run();
                                   },
                                   &status, loop.QuitClosure()));
    loop.Run();
  }
  output += "open " + std::to_string(status) + "\n";
  if (status == 0) {
    output += "duration " + (host.durations.empty() ? std::string("unset") : Time(host.durations.back())) + "\n";
    output += "start " + Time(demuxer->GetStartTime()) + "\n";
    std::vector<media::FFmpegDemuxerStream*> streams;
    for (media::DemuxerStream* stream : demuxer->GetAllStreams()) {
      streams.push_back(static_cast<media::FFmpegDemuxerStream*>(stream));
    }
    for (size_t i = 0; i < streams.size(); ++i) {
      media::FFmpegDemuxerStream* stream = streams[i];
      output += "stream " + std::to_string(i) + " liveness=" + std::to_string(static_cast<int>(stream->liveness())) +
                " enabled=" + std::to_string(stream->IsEnabled() ? 1 : 0) + " ";
      if (stream->type() == media::DemuxerStream::AUDIO) {
        output += "audio " + AudioConfig(stream->audio_decoder_config()) + "\n";
      } else {
        output += "video " + stream->video_decoder_config().AsHumanReadableString() + "\n";
      }
    }
    for (size_t i = 0; i < streams.size(); ++i) {
      media::FFmpegDemuxerStream* stream = streams[i];
      if (!stream->IsEnabled()) continue;
      output += "packets " + std::to_string(i) + "\n";
      for (;;) {
        base::RunLoop loop;
        media::DemuxerStream::Status read_status = media::DemuxerStream::kAborted;
        media::DemuxerStream::DecoderBufferVector buffers;
        stream->Read(1, base::BindOnce(
                            [](media::DemuxerStream::Status* status, media::DemuxerStream::DecoderBufferVector* out,
                               base::OnceClosure done, media::DemuxerStream::Status result,
                               media::DemuxerStream::DecoderBufferVector read) {
                              *status = result;
                              *out = std::move(read);
                              std::move(done).Run();
                            },
                            &read_status, &buffers, loop.QuitClosure()));
        loop.Run();
        if (read_status != media::DemuxerStream::kOk) {
          output += "status " + std::to_string(static_cast<int>(read_status)) + "\n";
          break;
        }
        if (buffers.size() != 1) {
          output += "count " + std::to_string(buffers.size()) + "\n";
          break;
        }
        if (buffers[0]->end_of_stream()) {
          output += "end\n";
          break;
        }
        output += Packet(*buffers[0]);
      }
    }
    output += "final " + Time(host.durations.back()) + "\n";
    output += "buffered";
    for (size_t i = 0; i < host.buffered.size(); ++i) {
      output += " " + Time(host.buffered.start(i)) + "-" + Time(host.buffered.end(i));
    }
    output += "\n";
  }
  for (int error : host.errors) output += "error " + std::to_string(error) + "\n";
  demuxer->Stop();
  demuxer.reset();
  environment.RunUntilIdle();
  return output;
}

bool ReadExact(char* data, size_t size) {
  std::cin.read(data, static_cast<std::streamsize>(size));
  return static_cast<size_t>(std::cin.gcount()) == size;
}

}  // namespace

int main(int argc, char** argv) {
  base::CommandLine::Init(argc, argv);
  TestTimeouts::Initialize();
  base::FeatureList::InitInstance("", "");
  av_log_set_level(AV_LOG_QUIET);
  base::ScopedMemoryConsumerRegistry<base::DummyMemoryConsumerRegistry> memory_consumers;
  base::test::TaskEnvironment environment;
  char magic[4];
  if (!ReadExact(magic, 4) || std::string(magic, 4) != "MGB1") return 1;
  std::cout.write("MGB1", 4).flush();
  for (;;) {
    std::cin.read(magic, 4);
    if (std::cin.gcount() == 0 && std::cin.eof()) return 0;
    if (std::cin.gcount() != 4) return 1;
    uint32_t size = (static_cast<uint32_t>(static_cast<uint8_t>(magic[0])) << 24) |
                    (static_cast<uint32_t>(static_cast<uint8_t>(magic[1])) << 16) |
                    (static_cast<uint32_t>(static_cast<uint8_t>(magic[2])) << 8) | static_cast<uint8_t>(magic[3]);
    std::string input(size, '\0');
    if (!ReadExact(input.data(), size)) return 1;
    std::string output = Execute(environment, input);
    uint32_t length = output.size();
    char header[4] = {static_cast<char>(length >> 24), static_cast<char>(length >> 16), static_cast<char>(length >> 8),
                      static_cast<char>(length)};
    std::cout.write(header, 4).write(output.data(), output.size()).flush();
  }
}
