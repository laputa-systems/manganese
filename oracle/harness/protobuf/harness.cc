#include <cstdint>
#include <cstdio>
#include <string>

#include "components/leveldb_proto/internal/proto/shared_db_metadata.pb.h"
#include "components/origin_trials/proto/db_trial_token.pb.h"
#include "content/browser/background_sync/background_sync.pb.h"
#include "content/browser/payments/payment_app.pb.h"

static uint32_t read32(const unsigned char* p) {
  return (uint32_t(p[0]) << 24) | (uint32_t(p[1]) << 16) | (uint32_t(p[2]) << 8) | p[3];
}

static void write32(unsigned char* p, uint32_t n) {
  p[0] = n >> 24; p[1] = n >> 16; p[2] = n >> 8; p[3] = n;
}

template <typename T> static std::string process(const std::string& bytes) {
  T message;
  if (!message.ParseFromString(bytes)) return std::string(1, '\0');
  std::string output(1, '\1');
  if (!message.AppendToString(&output)) return std::string(1, '\0');
  return output;
}

int main() {
  unsigned char header[4];
  if (fread(header, 1, 4, stdin) != 4 || std::string(reinterpret_cast<char*>(header), 4) != "MGB1") return 1;
  if (fwrite("MGB1", 1, 4, stdout) != 4 || fflush(stdout)) return 1;
  while (fread(header, 1, 1, stdin) == 1) {
    if (fread(header + 1, 1, 3, stdin) != 3) return 1;
    uint32_t size = read32(header);
    std::string input(size, '\0');
    if (fread(input.data(), 1, size, stdin) != size) return 1;
    std::string output(1, '\0');
    if (!input.empty()) {
      const std::string bytes = input.substr(1);
      if (input[0] == 0) output = process<leveldb_proto::SharedDBMetadataProto>(bytes);
      if (input[0] == 1) output = process<origin_trials_pb::TrialTokenDbEntries>(bytes);
      if (input[0] == 2) output = process<content::BackgroundSyncRegistrationsProto>(bytes);
      if (input[0] == 3) output = process<content::StoredPaymentInstrumentProto>(bytes);
    }
    write32(header, output.size());
    if (fwrite(header, 1, 4, stdout) != 4 || fwrite(output.data(), 1, output.size(), stdout) != output.size() || fflush(stdout)) return 1;
  }
  return ferror(stdin) ? 1 : 0;
}
