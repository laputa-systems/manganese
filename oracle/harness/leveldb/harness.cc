#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <filesystem>
#include <map>
#include <memory>
#include <string>
#include <string_view>
#include <vector>
#include <unistd.h>

#include "leveldb/db.h"
#include "leveldb/iterator.h"
#include "leveldb/write_batch.h"
#include "third_party/leveldatabase/env_chromium.h"

namespace {

class Input {
 public:
  explicit Input(std::string_view bytes) : bytes_(bytes) {}
  std::string_view Take(size_t size) {
    if (size > bytes_.size() - offset_) std::abort();
    auto result = bytes_.substr(offset_, size);
    offset_ += size;
    return result;
  }
  uint8_t Byte() { return static_cast<uint8_t>(Take(1)[0]); }
  uint16_t Short() {
    uint16_t result = Byte();
    return (result << 8) | Byte();
  }
  uint32_t Int() {
    uint32_t result = Short();
    return (result << 16) | Short();
  }
  uint64_t Long() {
    uint64_t result = Int();
    return (result << 32) | Int();
  }
  std::string_view Blob() { return Take(Int()); }
  bool Done() const { return offset_ == bytes_.size(); }
 private:
  std::string_view bytes_;
  size_t offset_ = 0;
};

void Int(std::string& out, uint32_t value) {
  for (int shift = 24; shift >= 0; shift -= 8) out.push_back(static_cast<char>(value >> shift));
}

void Blob(std::string& out, std::string_view value) {
  if (value.size() > UINT32_MAX) std::abort();
  Int(out, static_cast<uint32_t>(value.size()));
  out.append(value);
}

uint8_t Code(const leveldb::Status& status) {
  if (status.ok()) return 0;
  if (status.IsNotFound()) return 1;
  if (status.IsCorruption()) return 2;
  if (status.IsInvalidArgument()) return 4;
  if (status.IsNotSupportedError()) return 5;
  return 3;
}

bool NameOk(std::string_view name) {
  return !name.empty() && name != "." && name != ".." &&
         name.find('/') == name.npos && name.find('\\') == name.npos;
}

std::string Record(std::string_view request, const std::filesystem::path& root) {
  Input input(request);
  uint8_t flags = input.Byte();
  leveldb_env::Options options;
  options.create_if_missing = true;
  options.paranoid_checks = flags & 1;
  options.reuse_logs = flags & 2;
  options.compression = flags & 4 ? leveldb::kSnappyCompression : leveldb::kNoCompression;
  leveldb::ReadOptions read_options;
  read_options.verify_checksums = flags & 8;
  read_options.fill_cache = !(flags & 16);
  const auto path = root / "db";
  std::error_code error;
  std::filesystem::remove_all(path, error);
  if (error) std::abort();
  std::filesystem::create_directories(path, error);
  if (error) std::abort();
  for (unsigned i = 0, n = input.Short(); i < n; ++i) {
    auto name = input.Blob();
    auto contents = input.Blob();
    if (!NameOk(name)) std::abort();
    auto file = std::unique_ptr<FILE, decltype(&std::fclose)>(std::fopen((path / std::string(name)).c_str(), "wb"), &std::fclose);
    if (!file || std::fwrite(contents.data(), 1, contents.size(), file.get()) != contents.size()) std::abort();
  }
  std::unique_ptr<leveldb::DB> db;
  std::map<uint8_t, const leveldb::Snapshot*> snapshots;
  auto close = [&] {
    if (db) for (auto [id, snapshot] : snapshots) db->ReleaseSnapshot(snapshot);
    snapshots.clear();
    db.reset();
  };
  auto current = [&]() -> leveldb::DB& { if (!db) std::abort(); return *db; };
  auto read = [&](uint8_t id) {
    auto result = read_options;
    if (id != 255) {
      auto it = snapshots.find(id);
      if (it == snapshots.end()) std::abort();
      result.snapshot = it->second;
    }
    return result;
  };
  std::string out;
  for (unsigned i = 0, n = input.Short(); i < n; ++i) {
    auto operation = input.Byte();
    switch (operation) {
      case 0: {
        close();
        leveldb::DB* raw = nullptr;
        auto status = leveldb::DB::Open(options, path.string(), &raw);
        db.reset(raw);
        out.push_back(Code(status));
        break;
      }
      case 1:
      case 2:
      case 3: {
        leveldb::WriteOptions write_options;
        write_options.sync = input.Byte() != 0;
        leveldb::WriteBatch batch;
        unsigned count = operation == 3 ? input.Short() : 1;
        for (unsigned j = 0; j < count; ++j) {
          auto action = operation == 3 ? input.Byte() : operation;
          auto key = input.Blob();
          if (action == 1) { auto value = input.Blob(); batch.Put(leveldb::Slice(key.data(), key.size()), leveldb::Slice(value.data(), value.size())); }
          else if (action == 2) batch.Delete(leveldb::Slice(key.data(), key.size()));
          else std::abort();
        }
        out.push_back(Code(current().Write(write_options, &batch)));
        break;
      }
      case 4: {
        auto id = input.Byte();
        auto key = input.Blob();
        std::string value;
        auto status = current().Get(read(id), leveldb::Slice(key.data(), key.size()), &value);
        out.push_back(Code(status));
        if (status.ok()) Blob(out, value);
        break;
      }
      case 5: {
        auto id = input.Byte();
        if (id == 255) std::abort();
        auto old = snapshots.find(id);
        if (old != snapshots.end()) current().ReleaseSnapshot(old->second);
        snapshots[id] = current().GetSnapshot();
        out.push_back(0);
        break;
      }
      case 6: {
        auto id = input.Byte();
        auto it = snapshots.find(id);
        if (it != snapshots.end()) { current().ReleaseSnapshot(it->second); snapshots.erase(it); }
        out.push_back(0);
        break;
      }
      case 7: {
        auto id = input.Byte();
        auto direction = input.Byte();
        auto key = input.Blob();
        std::unique_ptr<leveldb::Iterator> it(current().NewIterator(read(id)));
        if (direction == 0) it->SeekToFirst();
        else if (direction == 1) it->SeekToLast();
        else if (direction == 2) it->Seek(leveldb::Slice(key.data(), key.size()));
        else std::abort();
        std::vector<std::pair<std::string, std::string>> entries;
        for (; it->Valid(); direction == 1 ? it->Prev() : it->Next())
          entries.emplace_back(it->key().ToString(), it->value().ToString());
        auto status = it->status();
        out.push_back(Code(status));
        if (status.ok()) {
          Int(out, static_cast<uint32_t>(entries.size()));
          for (const auto& [k, v] : entries) { Blob(out, k); Blob(out, v); }
        }
        break;
      }
      case 8: {
        auto from = input.Blob();
        auto to = input.Blob();
        leveldb::Slice low(from.data(), from.size()), high(to.data(), to.size());
        current().CompactRange(&low, &high);
        out.push_back(0);
        break;
      }
      case 9: close(); out.push_back(0); break;
      case 10: close(); out.push_back(Code(leveldb::RepairDB(path.string(), options))); break;
      case 11: close(); out.push_back(Code(leveldb::DestroyDB(path.string(), options))); break;
      case 12: {
        if (db) std::abort();
        auto name = input.Blob();
        auto length = input.Long();
        if (!NameOk(name)) std::abort();
        out.push_back(::truncate((path / std::string(name)).c_str(), static_cast<off_t>(length)) == 0 ? 0 : 3);
        break;
      }
      default: std::abort();
    }
  }
  if (!input.Done()) std::abort();
  close();
  return out;
}

int Serve() {
  const char* root = std::getenv("GOLDENS_SCRATCH");
  if (!root) return 2;
  char header[4];
  if (std::fread(header, 1, 4, stdin) != 4 || std::string_view(header, 4) != "MGB1") return 2;
  if (std::fwrite("MGB1", 1, 4, stdout) != 4 || std::fflush(stdout)) return 1;
  while (std::fread(header, 1, 4, stdin) == 4) {
    Input length(std::string_view(header, 4));
    std::string request(length.Int(), '\0');
    if (std::fread(request.data(), 1, request.size(), stdin) != request.size()) return 1;
    auto response = Record(request, root);
    std::string size;
    Int(size, static_cast<uint32_t>(response.size()));
    if (std::fwrite(size.data(), 1, size.size(), stdout) != size.size() ||
        std::fwrite(response.data(), 1, response.size(), stdout) != response.size() || std::fflush(stdout)) return 1;
  }
  return std::feof(stdin) ? 0 : 1;
}

int RunCli(int argc, char** argv) {
  if (argc != 3) return 2;
  leveldb_env::Options options;
  options.create_if_missing = true;
  options.paranoid_checks = true;
  if (std::string(argv[1]) == "repair") {
    leveldb::Status status = leveldb::RepairDB(argv[2], options);
    std::fprintf(stderr, "%s\n", status.ToString().c_str());
    return status.ok() ? 0 : 1;
  }
  leveldb::DB* raw = nullptr;
  leveldb::Status status = leveldb::DB::Open(options, argv[2], &raw);
  if (!status.ok()) {
    std::fprintf(stderr, "%s\n", status.ToString().c_str());
    return 1;
  }
  std::unique_ptr<leveldb::DB> db(raw);
  leveldb::ReadOptions read_options;
  read_options.verify_checksums = true;
  std::unique_ptr<leveldb::Iterator> it(db->NewIterator(read_options));
  for (it->SeekToFirst(); it->Valid(); it->Next()) {
    for (unsigned char c : it->key().ToString()) std::printf("%02x", c);
    std::printf(" ");
    for (unsigned char c : it->value().ToString()) std::printf("%02x", c);
    std::printf("\n");
  }
  status = it->status();
  if (!status.ok()) std::fprintf(stderr, "%s\n", status.ToString().c_str());
  return status.ok() ? 0 : 1;
}

}

int main(int argc, char** argv) {
  return argc == 1 ? Serve() : RunCli(argc, argv);
}
