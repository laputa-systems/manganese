#include <unistd.h>

#include <algorithm>
#include <charconv>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <filesystem>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

#include "base/at_exit.h"
#include "base/command_line.h"
#include "base/task/thread_pool/thread_pool_instance.h"
#include "leveldb/db.h"
#include "leveldb/filter_policy.h"
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
    uint16_t high = Byte();
    return static_cast<uint16_t>((high << 8) | Byte());
  }
  uint32_t Int() {
    uint32_t high = Short();
    return (high << 16) | Short();
  }
  uint64_t Long() {
    uint64_t high = Int();
    return (high << 32) | Int();
  }
  std::string_view Blob() { return Take(Int()); }
  leveldb::Slice SliceBlob() {
    auto blob = Blob();
    return leveldb::Slice(blob.data(), blob.size());
  }
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

constexpr uint8_t kClosed = 6;

uint8_t Code(const leveldb::Status& status) {
  if (status.ok()) return 0;
  if (status.IsNotFound()) return 1;
  if (status.IsCorruption()) return 2;
  if (status.IsIOError()) return 3;
  if (status.IsInvalidArgument()) return 4;
  if (status.IsNotSupportedError()) return 5;
  std::abort();
}

bool FixtureNameOk(std::string_view name) {
  return !name.empty() && name != "." && name != ".." && name.find('/') == name.npos;
}

bool Number(std::string_view digits, uint64_t& number) {
  if (digits.empty()) return false;
  auto [end, error] = std::from_chars(digits.data(), digits.data() + digits.size(), number);
  return error == std::errc() && end == digits.data() + digits.size();
}

std::optional<uint8_t> FileKind(std::string_view name, uint64_t& number) {
  if (name == "CURRENT") { number = 0; return 3; }
  if (name.starts_with("MANIFEST-")) return Number(name.substr(9), number) ? std::optional<uint8_t>(2) : std::nullopt;
  auto dot = name.find('.');
  if (dot == name.npos || !Number(name.substr(0, dot), number)) return std::nullopt;
  auto extension = name.substr(dot);
  if (extension == ".log") return 0;
  if (extension == ".ldb" || extension == ".sst") return 1;
  if (extension == ".dbtmp") return 4;
  return std::nullopt;
}

std::optional<std::filesystem::path> SelectFile(const std::filesystem::path& directory, uint8_t kind, uint8_t rank) {
  std::vector<std::pair<uint64_t, std::filesystem::path>> files;
  std::error_code error;
  for (const auto& entry : std::filesystem::directory_iterator(directory, error)) {
    uint64_t number = 0;
    auto found = FileKind(entry.path().filename().string(), number);
    if (found && *found == kind) files.emplace_back(number, entry.path());
  }
  std::sort(files.begin(), files.end(), [](const auto& a, const auto& b) { return a.first > b.first; });
  if (rank >= files.size()) return std::nullopt;
  return files[rank].second;
}

void Flush() { base::ThreadPoolInstance::Get()->FlushForTesting(); }

struct Request {
  leveldb_env::Options options;
  const leveldb::FilterPolicy* filter = nullptr;
  ~Request() { delete filter; }
};

leveldb::ReadOptions Read(uint8_t flags, const leveldb::Snapshot* snapshot) {
  leveldb::ReadOptions read;
  read.verify_checksums = flags & 1;
  read.fill_cache = flags & 2;
  read.snapshot = snapshot;
  return read;
}

void Entry(std::string& out, leveldb::Iterator& it) {
  out.push_back(1);
  Blob(out, std::string_view(it.key().data(), it.key().size()));
  Blob(out, std::string_view(it.value().data(), it.value().size()));
}

void Iterate(Input& input, leveldb::DB& db, const leveldb::ReadOptions& read, std::string& out) {
  std::unique_ptr<leveldb::Iterator> it(db.NewIterator(read));
  for (unsigned step = 0, steps = input.Byte(); step < steps; ++step) {
    auto movement = input.Byte();
    switch (movement) {
      case 0: it->SeekToFirst(); break;
      case 1: it->SeekToLast(); break;
      case 2: it->Seek(input.SliceBlob()); break;
      case 3: if (it->Valid()) it->Next(); break;
      case 4: if (it->Valid()) it->Prev(); break;
      case 5:
      case 6: {
        std::string entries;
        uint32_t count = 0;
        for (; it->Valid(); movement == 5 ? it->Next() : it->Prev(), ++count) {
          Blob(entries, std::string_view(it->key().data(), it->key().size()));
          Blob(entries, std::string_view(it->value().data(), it->value().size()));
        }
        Int(out, count);
        out += entries;
        continue;
      }
      default: std::abort();
    }
    if (it->Valid()) Entry(out, *it); else out.push_back(0);
  }
  out.push_back(Code(it->status()));
}

std::string Record(std::string_view bytes, const std::filesystem::path& root) {
  Input input(bytes);
  Request request;
  auto& options = request.options;
  auto flags = input.Byte();
  options.paranoid_checks = flags & 1;
  options.reuse_logs = flags & 2;
  options.compression = flags & 4 ? leveldb::kSnappyCompression : leveldb::kNoCompression;
  if (flags & 8) request.filter = leveldb::NewBloomFilterPolicy(10);
  options.filter_policy = request.filter;
  options.create_if_missing = flags & 16;
  options.error_if_exists = flags & 32;
  if (flags & ~63) std::abort();
  options.write_buffer_size = input.Int();
  options.max_open_files = static_cast<int>(input.Int());
  const auto path = root / "db";
  const auto rebuild = std::filesystem::path(leveldb_env::DatabaseNameForRewriteDB(path.string()));
  std::error_code error;
  std::filesystem::remove_all(path, error);
  if (error) std::abort();
  std::filesystem::remove_all(rebuild, error);
  if (error) std::abort();
  for (unsigned i = 0, n = input.Short(); i < n; ++i) {
    auto directory = input.Byte() ? rebuild : path;
    auto name = input.Blob();
    auto contents = input.Blob();
    if (!FixtureNameOk(name)) std::abort();
    std::filesystem::create_directories(directory, error);
    if (error) std::abort();
    std::unique_ptr<FILE, decltype(&std::fclose)> file(std::fopen((directory / std::string(name)).c_str(), "wb"), &std::fclose);
    if (!file || std::fwrite(contents.data(), 1, contents.size(), file.get()) != contents.size()) std::abort();
  }
  std::unique_ptr<leveldb::DB> db;
  std::map<uint8_t, const leveldb::Snapshot*> snapshots;
  auto close = [&] {
    for (auto [id, snapshot] : snapshots) db->ReleaseSnapshot(snapshot);
    snapshots.clear();
    db.reset();
    Flush();
  };
  auto snapshot = [&](uint8_t id) -> const leveldb::Snapshot* {
    if (id == 255) return nullptr;
    auto it = snapshots.find(id);
    if (it == snapshots.end()) std::abort();
    return it->second;
  };
  std::string out;
  for (unsigned i = 0, n = input.Short(); i < n; ++i) {
    auto operation = input.Byte();
    switch (operation) {
      case 0: {
        if (db) close();
        out.push_back(Code(leveldb_env::OpenDB(options, path.string(), &db)));
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
          auto key = input.SliceBlob();
          if (action == 1) batch.Put(key, input.SliceBlob());
          else if (action == 2) batch.Delete(key);
          else std::abort();
        }
        out.push_back(db ? Code(db->Write(write_options, &batch)) : kClosed);
        break;
      }
      case 4: {
        auto read_flags = input.Byte();
        auto id = input.Byte();
        auto key = input.SliceBlob();
        if (!db) { out.push_back(kClosed); break; }
        std::string value;
        auto status = db->Get(Read(read_flags, snapshot(id)), key, &value);
        out.push_back(Code(status));
        if (status.ok()) Blob(out, value);
        break;
      }
      case 5: {
        auto id = input.Byte();
        if (id == 255) std::abort();
        if (!db) { out.push_back(kClosed); break; }
        auto old = snapshots.find(id);
        if (old != snapshots.end()) db->ReleaseSnapshot(old->second);
        snapshots[id] = db->GetSnapshot();
        out.push_back(0);
        break;
      }
      case 6: {
        auto it = snapshots.find(input.Byte());
        if (it != snapshots.end()) { db->ReleaseSnapshot(it->second); snapshots.erase(it); }
        out.push_back(0);
        break;
      }
      case 7: {
        auto read_flags = input.Byte();
        auto id = input.Byte();
        if (!db) {
          for (unsigned step = 0, steps = input.Byte(); step < steps; ++step) if (input.Byte() == 2) input.Blob();
          out.push_back(kClosed);
          break;
        }
        out.push_back(0);
        Iterate(input, *db, Read(read_flags, snapshot(id)), out);
        break;
      }
      case 8: {
        auto bounds = input.Byte();
        std::optional<leveldb::Slice> begin, end;
        if (bounds & 1) begin = input.SliceBlob();
        if (bounds & 2) end = input.SliceBlob();
        if (!db) { out.push_back(kClosed); break; }
        db->CompactRange(begin ? &*begin : nullptr, end ? &*end : nullptr);
        out.push_back(0);
        break;
      }
      case 9: if (db) close(); out.push_back(0); break;
      case 10: if (db) close(); out.push_back(Code(leveldb::RepairDB(path.string(), options))); break;
      case 11: if (db) close(); out.push_back(Code(leveldb::DestroyDB(path.string(), options))); break;
      case 12:
      case 13:
      case 14: {
        if (db) std::abort();
        auto kind = input.Byte();
        auto rank = input.Byte();
        auto target = SelectFile(path, kind, rank);
        if (operation == 12) {
          auto keep = input.Long();
          if (!target) { out.push_back(1); break; }
          auto size = std::filesystem::file_size(*target);
          std::filesystem::resize_file(*target, std::min(keep, static_cast<uint64_t>(size)));
        } else if (operation == 13) {
          auto offset = input.Long();
          auto mask = input.Blob();
          if (!target) { out.push_back(1); break; }
          std::unique_ptr<FILE, decltype(&std::fclose)> file(std::fopen(target->c_str(), "r+b"), &std::fclose);
          if (!file) std::abort();
          auto size = std::filesystem::file_size(*target);
          for (size_t k = 0; k < mask.size() && offset + k < size; ++k) {
            if (std::fseek(file.get(), static_cast<long>(offset + k), SEEK_SET)) std::abort();
            int byte = std::fgetc(file.get());
            if (byte == EOF || std::fseek(file.get(), static_cast<long>(offset + k), SEEK_SET)) std::abort();
            if (std::fputc(byte ^ static_cast<uint8_t>(mask[k]), file.get()) == EOF) std::abort();
          }
        } else {
          if (!target) { out.push_back(1); break; }
          std::filesystem::remove(*target);
        }
        out.push_back(0);
        break;
      }
      case 15: {
        if (!db) { out.push_back(kClosed); break; }
        for (auto [id, held] : snapshots) db->ReleaseSnapshot(held);
        snapshots.clear();
        auto status = leveldb_env::RewriteDB(options, path.string(), &db);
        Flush();
        out.push_back(Code(status));
        break;
      }
      default: std::abort();
    }
    Flush();
  }
  if (!input.Done()) std::abort();
  if (db) close();
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

void Hex(std::string_view bytes) {
  for (unsigned char c : bytes) std::printf("%02x", c);
}

int ReadDatabase(const char* path) {
  leveldb_env::Options options;
  std::unique_ptr<leveldb::DB> db;
  auto status = leveldb_env::OpenDB(options, path, &db);
  if (!status.ok()) {
    std::printf("open %d\n", Code(status));
    return 0;
  }
  leveldb::ReadOptions read;
  read.verify_checksums = true;
  std::unique_ptr<leveldb::Iterator> it(db->NewIterator(read));
  for (it->SeekToFirst(); it->Valid(); it->Next()) {
    Hex(std::string_view(it->key().data(), it->key().size()));
    std::printf(" ");
    Hex(std::string_view(it->value().data(), it->value().size()));
    std::printf("\n");
  }
  std::printf("status %d\n", Code(it->status()));
  it.reset();
  db.reset();
  Flush();
  return 0;
}

}  // namespace

int main(int argc, char** argv) {
  base::AtExitManager exit_manager;
  base::CommandLine::Init(argc, argv);
  base::ThreadPoolInstance::CreateAndStartWithDefaultParams("leveldb");
  int result = 2;
  if (argc == 1) result = Serve();
  else if (argc == 3 && std::string_view(argv[1]) == "read") result = ReadDatabase(argv[2]);
  base::ThreadPoolInstance::Get()->Shutdown();
  return result;
}
