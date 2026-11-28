#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include <map>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <vector>

#include "base/at_exit.h"
#include "base/command_line.h"
#include "base/files/file_path.h"
#include "base/files/file_util.h"
#include "sql/database.h"
#include "sql/meta_table.h"
#include "sql/statement.h"
#include "sql/transaction.h"
#include "third_party/boringssl/src/include/openssl/sha.h"
#include "third_party/sqlite/sqlite3.h"

namespace {

enum Op : uint8_t {
  kFile = 0x01,
  kOpen = 0x02,
  kClose = 0x03,
  kExecute = 0x04,
  kPrepare = 0x05,
  kBind = 0x06,
  kStep = 0x07,
  kReset = 0x08,
  kFinalize = 0x09,
  kBegin = 0x0a,
  kCommit = 0x0b,
  kRollback = 0x0c,
  kEndTransaction = 0x0d,
  kMetaInit = 0x0e,
  kMetaSetVersion = 0x0f,
  kMetaGetVersion = 0x10,
  kMetaSetCompatible = 0x11,
  kMetaGetCompatible = 0x12,
  kMetaSetValue = 0x13,
  kMetaGetValue = 0x14,
  kMetaDeleteKey = 0x15,
  kMetaRazeIfIncompatible = 0x16,
  kRaze = 0x17,
  kRazeAndPoison = 0x18,
  kPoison = 0x19,
  kVacuum = 0x1a,
  kCheckpoint = 0x1b,
  kExists = 0x1c,
  kChanges = 0x1d,
  kDump = 0x1e,
  kArtifacts = 0x1f,
  kDigest = 0x20,
  kResidue = 0x21,
  kCrash = 0x22,
  kExport = 0x23,
  kSqlValid = 0x24,
};

enum ValueTag : uint8_t {
  kNull = 0,
  kInteger = 1,
  kFloat = 2,
  kText = 3,
  kBlob = 4,
};

const char* const kSuffixes[] = {"", "-journal", "-wal"};

[[noreturn]] void Fail(const char* message) {
  fprintf(stderr, "sqlite oracle: %s\n", message);
  exit(2);
}

class Reader {
 public:
  explicit Reader(const std::string& data) : data_(data) {}
  bool done() const { return pos_ == data_.size(); }
  uint8_t U8() {
    Need(1);
    return static_cast<uint8_t>(data_[pos_++]);
  }
  uint16_t U16() {
    uint16_t high = U8();
    return static_cast<uint16_t>(high << 8 | U8());
  }
  uint32_t U32() {
    uint32_t high = U16();
    return high << 16 | U16();
  }
  uint64_t U64() {
    uint64_t high = U32();
    return high << 32 | U32();
  }
  std::string Bytes() {
    uint32_t length = U32();
    Need(length);
    std::string result = data_.substr(pos_, length);
    pos_ += length;
    return result;
  }

 private:
  void Need(size_t count) {
    if (data_.size() - pos_ < count) {
      Fail("truncated request");
    }
  }
  const std::string& data_;
  size_t pos_ = 0;
};

class Writer {
 public:
  void U8(uint8_t value) { out_.push_back(static_cast<char>(value)); }
  void U16(uint16_t value) {
    U8(value >> 8);
    U8(value & 0xff);
  }
  void U32(uint32_t value) {
    U16(value >> 16);
    U16(value & 0xffff);
  }
  void U64(uint64_t value) {
    U32(value >> 32);
    U32(value & 0xffffffff);
  }
  void I32(int32_t value) { U32(static_cast<uint32_t>(value)); }
  void I64(int64_t value) { U64(static_cast<uint64_t>(value)); }
  void Bytes(std::string_view value) {
    U32(static_cast<uint32_t>(value.size()));
    out_.append(value);
  }
  void Bool(bool value) { U8(value ? 1 : 0); }
  const std::string& str() const { return out_; }

 private:
  std::string out_;
};

size_t ReadInput(char* data, size_t length) {
  size_t got = 0;
  while (got < length) {
    ssize_t count = read(STDIN_FILENO, data + got, length - got);
    if (count < 0 && errno == EINTR) {
      continue;
    }
    if (count < 0) {
      Fail("cannot read request");
    }
    if (count == 0) {
      break;
    }
    got += static_cast<size_t>(count);
  }
  return got;
}

std::string ReadBody(const unsigned char* header) {
  uint32_t length = uint32_t{header[0]} << 24 | uint32_t{header[1]} << 16 |
                    uint32_t{header[2]} << 8 | header[3];
  std::string body(length, '\0');
  if (length && ReadInput(body.data(), length) != length) {
    Fail("truncated frame body");
  }
  return body;
}

void WriteFrame(const std::string& body) {
  unsigned char header[4] = {
      static_cast<unsigned char>(body.size() >> 24),
      static_cast<unsigned char>(body.size() >> 16),
      static_cast<unsigned char>(body.size() >> 8),
      static_cast<unsigned char>(body.size())};
  if (fwrite(header, 1, 4, stdout) != 4 ||
      fwrite(body.data(), 1, body.size(), stdout) != body.size() ||
      fflush(stdout) != 0) {
    Fail("cannot write response");
  }
}

void WriteValue(Writer& out, sql::Statement& statement, int column,
                uint8_t getter) {
  switch (getter) {
    case 1:
      out.U8(kInteger);
      out.I64(statement.ColumnInt64(column));
      return;
    case 2: {
      out.U8(kFloat);
      double value = statement.ColumnDouble(column);
      uint64_t bits;
      memcpy(&bits, &value, sizeof(bits));
      out.U64(bits);
      return;
    }
    case 3:
      out.U8(kText);
      out.Bytes(statement.ColumnString(column));
      return;
    case 4:
      out.U8(kBlob);
      out.Bytes(statement.ColumnBlobAsString(column));
      return;
    case 5:
      out.U8(kInteger);
      out.I64(statement.ColumnInt(column));
      return;
    case 6:
      out.U8(kInteger);
      out.I64(statement.ColumnBool(column) ? 1 : 0);
      return;
  }
  switch (statement.GetColumnType(column)) {
    case sql::ColumnType::kInteger:
      WriteValue(out, statement, column, 1);
      return;
    case sql::ColumnType::kFloat:
      WriteValue(out, statement, column, 2);
      return;
    case sql::ColumnType::kText:
      WriteValue(out, statement, column, 3);
      return;
    case sql::ColumnType::kBlob:
      WriteValue(out, statement, column, 4);
      return;
    case sql::ColumnType::kNull:
      out.U8(kNull);
      return;
  }
}

std::string Sha256(const std::string& data) {
  uint8_t digest[SHA256_DIGEST_LENGTH];
  SHA256(reinterpret_cast<const uint8_t*>(data.data()), data.size(), digest);
  return std::string(reinterpret_cast<const char*>(digest), sizeof(digest));
}

std::optional<std::string> ReadWholeFile(const base::FilePath& path) {
  std::string contents;
  if (!base::PathExists(path) || !base::ReadFileToString(path, &contents)) {
    return std::nullopt;
  }
  return contents;
}

class Session {
 public:
  explicit Session(base::FilePath root) : root_(std::move(root)) {}

  ~Session() {
    transactions_.clear();
    statements_.clear();
    meta_.reset();
    database_.reset();
    abandoned_.clear();
    base::DeletePathRecursively(root_);
  }

  std::string Run(const std::string& request) {
    Reader in(request);
    Writer out;
    if (!base::CreateDirectory(Directory())) {
      Fail("cannot create request directory");
    }
    uint16_t steps = in.U16();
    for (uint16_t index = 0; index < steps; ++index) {
      Step(in, out);
    }
    if (!in.done()) {
      Fail("trailing request bytes");
    }
    return out.str();
  }

 private:
  base::FilePath Directory() const {
    return root_.AppendASCII("g" + std::to_string(generation_));
  }
  base::FilePath DatabasePath() const { return Directory().AppendASCII("db"); }
  base::FilePath SidecarPath(int kind) const {
    return base::FilePath(DatabasePath().value() + kSuffixes[kind]);
  }

  sql::Database& Db() {
    if (!database_) {
      Fail("no database object");
    }
    return *database_;
  }

  sql::Statement& Slot(uint8_t slot) {
    auto found = statements_.find(slot);
    if (found == statements_.end()) {
      Fail("unknown statement slot");
    }
    return *found->second;
  }

  sql::MetaTable& Meta() {
    if (!meta_) {
      Fail("meta table not initialized");
    }
    return *meta_;
  }

  void Result(Writer& out, bool ok) {
    out.Bool(ok);
    out.I32(ok ? 0 : Db().GetErrorCode());
  }

  void Step(Reader& in, Writer& out) {
    uint8_t op = in.U8();
    out.U8(op);
    switch (op) {
      case kFile: {
        uint8_t kind = in.U8();
        if (kind > 2) {
          Fail("unknown file kind");
        }
        std::string contents = in.Bytes();
        if (!base::WriteFile(SidecarPath(kind), contents)) {
          Fail("cannot write database file");
        }
        out.Bool(true);
        return;
      }
      case kOpen: {
        uint8_t flags = in.U8();
        uint32_t page_size = in.U32();
        int32_t cache_size = static_cast<int32_t>(in.U32());
        sql::DatabaseOptions options;
        options.set_wal_mode(flags & 0x01)
            .set_exclusive_locking(!(flags & 0x02))
            .set_read_only(flags & 0x04)
            .set_mmap_enabled(!(flags & 0x08))
            .set_enable_triggers(flags & 0x10)
            .set_enable_views_discouraged(flags & 0x20)
            .set_no_sync(flags & 0x40)
            .set_mmap_alt_status_discouraged(flags & 0x80)
            .set_page_size(static_cast<int>(page_size))
            .set_cache_size(cache_size);
        transactions_.clear();
        statements_.clear();
        stepped_.clear();
        meta_.reset();
        database_ = std::make_unique<sql::Database>(std::move(options),
                                                    sql::Database::Tag("Test"));
        Result(out, database_->Open(DatabasePath()));
        return;
      }
      case kClose:
        transactions_.clear();
        statements_.clear();
        stepped_.clear();
        meta_.reset();
        Db().Close();
        out.Bool(true);
        return;
      case kExecute: {
        std::string sql = in.Bytes();
        Result(out, Db().Execute(sql));
        return;
      }
      case kPrepare: {
        uint8_t slot = in.U8();
        std::string sql = in.Bytes();
        auto statement =
            std::make_unique<sql::Statement>(Db().GetUniqueStatement(sql));
        bool valid = statement->is_valid();
        stepped_.erase(slot);
        statements_[slot] = std::move(statement);
        Result(out, valid);
        return;
      }
      case kBind: {
        uint8_t slot = in.U8();
        int index = in.U16();
        uint8_t tag = in.U8();
        uint64_t bits = 0;
        std::string bytes;
        switch (tag) {
          case kNull:
            break;
          case kInteger:
          case kFloat:
            bits = in.U64();
            break;
          case kText:
          case kBlob:
            bytes = in.Bytes();
            break;
          default:
            Fail("unknown value tag");
        }
        if (!stepped_.count(slot)) {
          sql::Statement& statement = Slot(slot);
          switch (tag) {
            case kNull:
              statement.BindNull(index);
              break;
            case kInteger:
              statement.BindInt64(index, static_cast<int64_t>(bits));
              break;
            case kFloat: {
              double value;
              memcpy(&value, &bits, sizeof(value));
              statement.BindDouble(index, value);
              break;
            }
            case kText:
              statement.BindString(index, bytes);
              break;
            case kBlob:
              statement.BindBlob(index, bytes);
              break;
            default:
              Fail("unknown value tag");
          }
        }
        out.Bool(true);
        return;
      }
      case kStep: {
        uint8_t slot = in.U8();
        sql::Statement& statement = Slot(slot);
        stepped_.insert(slot);
        uint16_t max_rows = in.U16();
        uint8_t getter_count = in.U8();
        std::vector<uint8_t> getters;
        for (uint8_t index = 0; index < getter_count; ++index) {
          getters.push_back(in.U8());
        }
        Writer rows;
        uint32_t row_count = 0;
        bool more = false;
        while (row_count < max_rows) {
          more = statement.Step();
          if (!more) {
            break;
          }
          ++row_count;
          int columns = statement.ColumnCount();
          rows.U16(static_cast<uint16_t>(columns));
          for (int column = 0; column < columns; ++column) {
            WriteValue(rows, statement, column,
                       column < static_cast<int>(getters.size())
                           ? getters[column]
                           : 0);
          }
        }
        out.U32(row_count);
        out.Bytes(rows.str());
        out.Bool(more);
        bool succeeded = statement.Succeeded();
        Result(out, succeeded || more);
        return;
      }
      case kReset: {
        uint8_t slot = in.U8();
        sql::Statement& statement = Slot(slot);
        stepped_.erase(slot);
        statement.Reset(in.U8() != 0);
        out.Bool(true);
        return;
      }
      case kFinalize: {
        uint8_t slot = in.U8();
        stepped_.erase(slot);
        statements_.erase(slot);
        out.Bool(true);
        return;
      }
      case kBegin: {
        uint8_t slot = in.U8();
        auto transaction = std::make_unique<sql::Transaction>(&Db());
        bool ok = transaction->Begin();
        transactions_[slot] = std::move(transaction);
        Result(out, ok);
        return;
      }
      case kCommit: {
        auto found = transactions_.find(in.U8());
        if (found == transactions_.end()) {
          Fail("unknown transaction slot");
        }
        Result(out, found->second->Commit());
        return;
      }
      case kRollback: {
        auto found = transactions_.find(in.U8());
        if (found == transactions_.end()) {
          Fail("unknown transaction slot");
        }
        found->second->Rollback();
        out.Bool(true);
        return;
      }
      case kEndTransaction:
        transactions_.erase(in.U8());
        out.Bool(true);
        return;
      case kMetaInit: {
        int32_t version = static_cast<int32_t>(in.U32());
        int32_t compatible = static_cast<int32_t>(in.U32());
        meta_ = std::make_unique<sql::MetaTable>();
        Result(out, meta_->Init(&Db(), version, compatible));
        return;
      }
      case kMetaSetVersion:
        Result(out, Meta().SetVersionNumber(static_cast<int32_t>(in.U32())));
        return;
      case kMetaGetVersion:
        out.Bool(true);
        out.I32(Meta().GetVersionNumber());
        return;
      case kMetaSetCompatible:
        Result(out,
               Meta().SetCompatibleVersionNumber(static_cast<int32_t>(in.U32())));
        return;
      case kMetaGetCompatible:
        out.Bool(true);
        out.I32(Meta().GetCompatibleVersionNumber());
        return;
      case kMetaSetValue: {
        std::string key = in.Bytes();
        uint8_t tag = in.U8();
        if (tag == kInteger) {
          Result(out, Meta().SetValue(key, static_cast<int64_t>(in.U64())));
        } else if (tag == kText) {
          Result(out, Meta().SetValue(key, in.Bytes()));
        } else {
          Fail("meta values are integers or text");
        }
        return;
      }
      case kMetaGetValue: {
        std::string key = in.Bytes();
        uint8_t tag = in.U8();
        if (tag == kInteger) {
          int64_t value = 0;
          bool ok = Meta().GetValue(key, &value);
          out.Bool(ok);
          out.I64(value);
        } else if (tag == kText) {
          std::string value;
          bool ok = Meta().GetValue(key, &value);
          out.Bool(ok);
          out.Bytes(value);
        } else {
          Fail("meta values are integers or text");
        }
        return;
      }
      case kMetaDeleteKey:
        Result(out, Meta().DeleteKey(in.Bytes()));
        return;
      case kMetaRazeIfIncompatible: {
        int32_t lowest = static_cast<int32_t>(in.U32());
        int32_t current = static_cast<int32_t>(in.U32());
        out.Bool(true);
        out.U8(static_cast<uint8_t>(
            sql::MetaTable::RazeIfIncompatible(&Db(), lowest, current)));
        return;
      }
      case kRaze:
        Result(out, Db().Raze());
        return;
      case kRazeAndPoison:
        transactions_.clear();
        statements_.clear();
        stepped_.clear();
        meta_.reset();
        out.Bool(Db().RazeAndPoison());
        return;
      case kPoison:
        Db().Poison();
        out.Bool(true);
        return;
      case kVacuum:
        Result(out, Db().Vacuum());
        return;
      case kCheckpoint:
        out.Bool(Db().CheckpointDatabase(in.U8() != 0));
        return;
      case kExists: {
        uint8_t kind = in.U8();
        std::string name = in.Bytes();
        bool exists = false;
        if (kind == 0) {
          exists = Db().DoesTableExist(name);
        } else if (kind == 1) {
          exists = Db().DoesIndexExist(name);
        } else if (kind == 2) {
          exists = Db().DoesViewExist(name);
        } else if (kind == 3) {
          std::string column = in.Bytes();
          exists = Db().DoesColumnExist(name, column);
        } else if (kind == 4) {
          exists = sql::MetaTable::DoesTableExist(&Db());
        } else {
          Fail("unknown schema item kind");
        }
        out.Bool(exists);
        return;
      }
      case kChanges:
        out.Bool(true);
        out.I64(Db().GetLastChangeCount());
        out.I64(Db().GetLastInsertRowId());
        return;
      case kDump:
        Dump(out);
        return;
      case kArtifacts:
        out.Bool(true);
        for (int kind = 0; kind < 3; ++kind) {
          std::optional<int64_t> size = base::GetFileSize(SidecarPath(kind));
          out.Bool(size.has_value());
          out.I64(size.value_or(0));
        }
        return;
      case kDigest: {
        std::optional<std::string> contents = ReadWholeFile(DatabasePath());
        out.Bool(contents.has_value());
        out.Bytes(contents ? Sha256(*contents) : std::string());
        return;
      }
      case kResidue: {
        std::string pattern = in.Bytes();
        bool found = false;
        for (int kind = 0; kind < 3; ++kind) {
          std::optional<std::string> contents =
              ReadWholeFile(SidecarPath(kind));
          if (contents && contents->find(pattern) != std::string::npos) {
            found = true;
          }
        }
        out.Bool(true);
        out.Bool(found);
        return;
      }
      case kCrash: {
        uint8_t action = in.U8();
        std::vector<std::optional<std::string>> files;
        for (int kind = 0; kind < 3; ++kind) {
          files.push_back(ReadWholeFile(SidecarPath(kind)));
        }
        if (database_) {
          abandoned_.push_back(std::move(database_));
        }
        transactions_.clear();
        statements_.clear();
        meta_.reset();
        ++generation_;
        if (!base::CreateDirectory(Directory())) {
          Fail("cannot create crash directory");
        }
        for (int kind = 0; kind < 3; ++kind) {
          if (!files[kind]) {
            continue;
          }
          if (kind > 0 && action == 1) {
            continue;
          }
          if (!base::WriteFile(SidecarPath(kind), *files[kind])) {
            Fail("cannot write crash image");
          }
        }
        out.Bool(true);
        return;
      }
      case kExport:
        out.Bool(true);
        for (int kind = 0; kind < 3; ++kind) {
          std::optional<std::string> contents =
              ReadWholeFile(SidecarPath(kind));
          out.Bool(contents.has_value());
          out.Bytes(contents.value_or(std::string()));
        }
        return;
      case kSqlValid:
        out.Bool(Db().IsSQLValid(in.Bytes()));
        return;
      default:
        Fail("unknown step op");
    }
  }

  void DumpQuery(Writer& out, const std::string& sql) {
    sql::Statement statement(Db().GetUniqueStatement(sql));
    Writer rows;
    uint32_t count = 0;
    while (statement.Step()) {
      ++count;
      int columns = statement.ColumnCount();
      rows.U16(static_cast<uint16_t>(columns));
      for (int column = 0; column < columns; ++column) {
        WriteValue(rows, statement, column, 0);
      }
    }
    out.Bool(statement.Succeeded());
    out.U32(count);
    out.Bytes(rows.str());
  }

  void Dump(Writer& out) {
    out.Bool(true);
    DumpQuery(out, "PRAGMA page_size");
    DumpQuery(out, "PRAGMA user_version");
    DumpQuery(out, "PRAGMA application_id");
    DumpQuery(out,
              "SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY "
              "rowid");
    std::vector<std::pair<std::string, bool>> tables;
    {
      sql::Statement statement(Db().GetUniqueStatement(
          "SELECT name, sql FROM sqlite_schema WHERE type = 'table' ORDER BY "
          "rowid"));
      while (statement.Step()) {
        std::string sql = statement.ColumnString(1);
        if (sql.starts_with("CREATE VIRTUAL TABLE")) {
          continue;
        }
        bool without_rowid = sql.find("WITHOUT ROWID") != std::string::npos ||
                             sql.find("without rowid") != std::string::npos;
        tables.emplace_back(statement.ColumnString(0), without_rowid);
      }
    }
    out.U32(static_cast<uint32_t>(tables.size()));
    for (const auto& [name, without_rowid] : tables) {
      std::string quoted = "\"";
      for (char c : name) {
        quoted.push_back(c);
        if (c == '"') {
          quoted.push_back('"');
        }
      }
      quoted.push_back('"');
      out.Bytes(name);
      DumpQuery(out, "SELECT * FROM " + quoted +
                         (without_rowid ? std::string() : " ORDER BY rowid"));
    }
  }

  base::FilePath root_;
  int generation_ = 0;
  std::unique_ptr<sql::Database> database_;
  std::vector<std::unique_ptr<sql::Database>> abandoned_;
  std::map<uint8_t, std::unique_ptr<sql::Statement>> statements_;
  std::set<uint8_t> stepped_;
  std::map<uint8_t, std::unique_ptr<sql::Transaction>> transactions_;
  std::unique_ptr<sql::MetaTable> meta_;
};

[[noreturn]] void ServeRequest(const char* scratch,
                               const unsigned char* header) {
  std::string request = ReadBody(header);
  std::string response;
  {
    Session session(base::FilePath(scratch).AppendASCII("r0"));
    response = session.Run(request);
  }
  WriteFrame(response);
  _exit(0);
}

}  // namespace

int main(int argc, char** argv) {
  base::AtExitManager exit_manager;
  base::CommandLine::Init(argc, argv);
  const char* scratch = getenv("GOLDENS_SCRATCH");
  if (!scratch || !*scratch) {
    Fail("GOLDENS_SCRATCH is not set");
  }
  char magic[4];
  if (ReadInput(magic, 4) != 4 || memcmp(magic, "MGB1", 4) != 0) {
    Fail("missing MGB1 header");
  }
  if (fwrite("MGB1", 1, 4, stdout) != 4 || fflush(stdout) != 0) {
    Fail("cannot write header");
  }
  // Each request runs in a child forked from this process, which allocates
  // nothing after the handshake, so its heap depends only on the request and
  // not on the requests that came before it in the stream.
  for (;;) {
    unsigned char header[4];
    size_t got = ReadInput(reinterpret_cast<char*>(header), 4);
    if (got == 0) {
      return 0;
    }
    if (got != 4) {
      Fail("truncated frame header");
    }
    pid_t child = fork();
    if (child < 0) {
      Fail("cannot start a request process");
    }
    if (child == 0) {
      ServeRequest(scratch, header);
    }
    int status = 0;
    while (waitpid(child, &status, 0) < 0) {
      if (errno != EINTR) {
        Fail("cannot wait for a request process");
      }
    }
    if (WIFSIGNALED(status)) {
      fprintf(stderr, "sqlite oracle: request process killed by signal %d\n",
              WTERMSIG(status));
      exit(2);
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
      exit(WIFEXITED(status) ? WEXITSTATUS(status) : 2);
    }
  }
}
