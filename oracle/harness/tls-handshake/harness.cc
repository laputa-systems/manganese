#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <iostream>
#include <memory>
#include <string>
#include <utility>
#include <vector>

#include <openssl/base.h>
#include <openssl/bio.h>
#include <openssl/err.h>
#include <openssl/ssl.h>
#include <openssl/tls1.h>

namespace {

constexpr uint16_t kGrease = 0xfaca;
constexpr uint16_t kMaxHost = 253;
constexpr uint16_t kMaxProto = 32;
constexpr uint16_t kMaxAlpn = 8;

struct Record {
  std::string host;
  std::vector<std::string> alpn;
};

class Reader {
 public:
  explicit Reader(const std::vector<uint8_t>& data) : data_(data) {}
  bool U8(uint8_t* out) {
    if (offset_ >= data_.size()) return false;
    *out = data_[offset_++];
    return true;
  }
  bool U16(uint16_t* out) {
    if (offset_ + 2 > data_.size()) return false;
    *out = static_cast<uint16_t>((data_[offset_] << 8) | data_[offset_ + 1]);
    offset_ += 2;
    return true;
  }
  bool Bytes(size_t size, std::string* out) {
    if (offset_ + size > data_.size()) return false;
    out->assign(reinterpret_cast<const char*>(data_.data() + offset_), size);
    offset_ += size;
    return true;
  }
  bool Empty() const { return offset_ == data_.size(); }

 private:
  const std::vector<uint8_t>& data_;
  size_t offset_ = 0;
};

bool Decode(const std::vector<uint8_t>& input, Record* record) {
  Reader reader(input);
  uint8_t version = 0;
  uint16_t host_size = 0;
  uint8_t count = 0;
  if (!reader.U8(&version) || version != 1) return false;
  if (!reader.U16(&host_size) || host_size < 1 || host_size > kMaxHost) return false;
  if (!reader.Bytes(host_size, &record->host)) return false;
  if (!reader.U8(&count) || count > kMaxAlpn) return false;
  for (uint8_t i = 0; i < count; ++i) {
    uint8_t size = 0;
    std::string proto;
    if (!reader.U8(&size) || size < 1 || size > kMaxProto) return false;
    if (!reader.Bytes(size, &proto)) return false;
    record->alpn.push_back(std::move(proto));
  }
  return reader.Empty();
}

bool IsGrease(uint16_t value) {
  uint8_t low = value & 0xff;
  return (value >> 8) == low && (low & 0x0f) == 0x0a;
}

uint16_t NormalizeCode(uint16_t value) { return IsGrease(value) ? kGrease : value; }

struct Extension {
  uint16_t type;
  std::vector<uint8_t> body;
};

void NormalizeBody(uint16_t type, std::vector<uint8_t>* body) {
  if (IsGrease(type)) {
    std::fill(body->begin(), body->end(), 0);
  } else if (type == 0xfe0d) {
    body->assign(1, 0);
  } else if (type == 0x0033) {
    if (body->size() < 2) return;
    size_t offset = 2;
    std::vector<uint8_t> out = {(*body)[0], (*body)[1]};
    while (offset + 4 <= body->size()) {
      uint16_t group = static_cast<uint16_t>(((*body)[offset] << 8) | (*body)[offset + 1]);
      uint16_t size = static_cast<uint16_t>(((*body)[offset + 2] << 8) | (*body)[offset + 3]);
      if (offset + 4 + size > body->size()) break;
      group = NormalizeCode(group);
      out.push_back(group >> 8);
      out.push_back(group & 0xff);
      out.push_back((*body)[offset + 2]);
      out.push_back((*body)[offset + 3]);
      out.insert(out.end(), size, 0);
      offset += 4 + size;
    }
    *body = std::move(out);
  } else if (type == 0x000d || type == 0x000a || type == 0x002b) {
    const size_t start = type == 0x002b ? 1 : 2;
    if (body->size() >= start + 2) {
      uint16_t value = static_cast<uint16_t>(((*body)[start] << 8) | (*body)[start + 1]);
      value = NormalizeCode(value);
      (*body)[start] = value >> 8;
      (*body)[start + 1] = value & 0xff;
    }
  }
}

bool Canonicalize(const std::vector<uint8_t>& handshake, std::vector<uint8_t>* out) {
  if (handshake.size() < 4 || handshake[0] != 1) return false;
  size_t body_size = (static_cast<size_t>(handshake[1]) << 16) | (static_cast<size_t>(handshake[2]) << 8) | handshake[3];
  if (body_size + 4 != handshake.size()) return false;
  const std::vector<uint8_t> body(handshake.begin() + 4, handshake.end());

  size_t cursor = 0;
  auto take = [&](size_t size, std::vector<uint8_t>* sink) {
    if (cursor + size > body.size()) return false;
    if (sink != nullptr) sink->assign(body.begin() + cursor, body.begin() + cursor + size);
    cursor += size;
    return true;
  };
  if (!take(34, nullptr)) return false;
  uint8_t session_size = 0;
  if (!take(1, nullptr) || (cursor + (session_size = body[cursor - 1])) > body.size()) return false;
  cursor += session_size;
  uint16_t cipher_size = 0;
  if (cursor + 2 > body.size()) return false;
  cipher_size = static_cast<uint16_t>((body[cursor] << 8) | body[cursor + 1]);
  cursor += 2;
  if (cipher_size % 2 || cursor + cipher_size > body.size()) return false;
  std::vector<uint16_t> ciphers;
  for (uint16_t i = 0; i < cipher_size; i += 2) {
    ciphers.push_back(NormalizeCode(static_cast<uint16_t>((body[cursor + i] << 8) | body[cursor + i + 1])));
  }
  cursor += cipher_size;
  uint8_t compression_size = 0;
  if (!take(1, nullptr) || cursor + (compression_size = body[cursor - 1]) > body.size()) return false;
  std::vector<uint8_t> compression;
  if (!take(compression_size, &compression)) return false;
  uint16_t extension_size = 0;
  if (cursor + 2 > body.size()) return false;
  extension_size = static_cast<uint16_t>((body[cursor] << 8) | body[cursor + 1]);
  cursor += 2;
  if (cursor + extension_size != body.size()) return false;
  std::vector<Extension> extensions;
  while (cursor + 4 <= body.size()) {
    Extension extension;
    extension.type = NormalizeCode(static_cast<uint16_t>((body[cursor] << 8) | body[cursor + 1]));
    uint16_t size = static_cast<uint16_t>((body[cursor + 2] << 8) | body[cursor + 3]);
    cursor += 4;
    if (cursor + size > body.size()) return false;
    extension.body.assign(body.begin() + cursor, body.begin() + cursor + size);
    cursor += size;
    NormalizeBody(extension.type, &extension.body);
    extensions.push_back(std::move(extension));
  }
  if (cursor != body.size()) return false;
  std::sort(extensions.begin(), extensions.end(), [](const Extension& a, const Extension& b) {
    return a.type != b.type ? a.type < b.type : a.body < b.body;
  });

  out->clear();
  out->push_back(1);
  out->push_back(ciphers.size() >> 8);
  out->push_back(ciphers.size() & 0xff);
  for (uint16_t cipher : ciphers) {
    out->push_back(cipher >> 8);
    out->push_back(cipher & 0xff);
  }
  out->push_back(compression.size());
  out->insert(out->end(), compression.begin(), compression.end());
  out->push_back(extensions.size() >> 8);
  out->push_back(extensions.size() & 0xff);
  for (const Extension& extension : extensions) {
    out->push_back(extension.type >> 8);
    out->push_back(extension.type & 0xff);
    out->push_back(extension.body.size() >> 8);
    out->push_back(extension.body.size() & 0xff);
    out->insert(out->end(), extension.body.begin(), extension.body.end());
  }
  return true;
}

void Append(std::vector<uint8_t>* out, const std::string& value) {
  out->insert(out->end(), value.begin(), value.end());
}

std::vector<uint8_t>* g_client_hello = nullptr;

void MessageCallback(int is_write, int version, int content_type, const void* buf, size_t len, SSL*, void*) {
  if (is_write && content_type == 22 && len >= 4) {
    const uint8_t* bytes = static_cast<const uint8_t*>(buf);
    if (bytes[0] == 1) g_client_hello->assign(bytes, bytes + len);
  }
}

bool Generate(const Record& record, std::vector<uint8_t>* out) {
  bssl::UniquePtr<SSL_CTX> ctx(SSL_CTX_new(TLS_client_method()));
  if (!ctx) return false;
  g_client_hello->clear();
  SSL_CTX_set_msg_callback(ctx.get(), MessageCallback);
  SSL_CTX_set_grease_enabled(ctx.get(), 1);
  SSL_CTX_set_grease_sigalgs_enabled(ctx.get(), 1);
  SSL_CTX_set_session_cache_mode(ctx.get(), SSL_SESS_CACHE_CLIENT | SSL_SESS_CACHE_NO_INTERNAL);
  SSL_CTX_set_timeout(ctx.get(), 3600);
  SSL_CTX_add_cert_compression_alg(ctx.get(), TLSEXT_cert_compression_brotli, nullptr,
                                   [](SSL*, CRYPTO_BUFFER**, size_t, const uint8_t*, size_t) { return 0; });
  bssl::UniquePtr<SSL> ssl(SSL_new(ctx.get()));
  if (!ssl) return false;
  if (!SSL_set_tlsext_host_name(ssl.get(), record.host.c_str())) return false;
  const uint16_t groups[] = {SSL_GROUP_X25519_MLKEM768, SSL_GROUP_X25519, SSL_GROUP_SECP256R1, SSL_GROUP_SECP384R1};
  const uint16_t key_shares[] = {SSL_GROUP_X25519_MLKEM768, SSL_GROUP_X25519};
  if (!SSL_set1_group_ids(ssl.get(), groups, 4) ||
      !SSL_set1_client_key_shares(ssl.get(), key_shares, 2)) return false;
  if (!SSL_set_min_proto_version(ssl.get(), TLS1_2_VERSION) ||
      !SSL_set_max_proto_version(ssl.get(), TLS1_3_VERSION)) return false;
  if (!SSL_set_strict_cipher_list(ssl.get(), "ALL:!aPSK:!ECDSA+SHA1:!3DES")) return false;
  const uint16_t prefs[] = {SSL_SIGN_ML_DSA_44, 0x0905, 0x0906,
                            0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806, 0x0601};
  if (!SSL_set_verify_algorithm_prefs(ssl.get(), prefs, 11)) return false;
  if (!record.alpn.empty()) {
    std::vector<uint8_t> wire;
    for (const std::string& proto : record.alpn) {
      wire.push_back(static_cast<uint8_t>(proto.size()));
      Append(&wire, proto);
    }
    if (SSL_set_alpn_protos(ssl.get(), wire.data(), wire.size())) return false;
  }
  SSL_enable_signed_cert_timestamps(ssl.get());
  SSL_enable_ocsp_stapling(ssl.get());
  SSL_set_shed_handshake_config(ssl.get(), 1);
  SSL_set_enable_ech_grease(ssl.get(), 1);
  SSL_set_permute_extensions(ssl.get(), 1);

  BIO* read_bio = nullptr;
  BIO* write_bio = nullptr;
  if (!BIO_new_bio_pair(&read_bio, 0, &write_bio, 0)) return false;
  SSL_set_bio(ssl.get(), write_bio, write_bio);
  SSL_set_connect_state(ssl.get());
  ERR_clear_error();
  SSL_do_handshake(ssl.get());
  return Canonicalize(*g_client_hello, out);
}

}  // namespace

int main() {
  std::vector<uint8_t> client_hello;
  g_client_hello = &client_hello;
  uint8_t magic[4];
  if (std::fread(magic, 1, 4, stdin) != 4 || std::memcmp(magic, "MGB1", 4) != 0) return 1;
  if (std::fwrite("MGB1", 1, 4, stdout) != 4) return 1;
  std::fflush(stdout);
  for (;;) {
    uint8_t header[4];
    if (std::fread(header, 1, 4, stdin) != 4) return std::feof(stdin) ? 0 : 1;
    uint32_t size = (static_cast<uint32_t>(header[0]) << 24) | (static_cast<uint32_t>(header[1]) << 16) |
                    (static_cast<uint32_t>(header[2]) << 8) | header[3];
    if (size > (1u << 20)) return 1;
    std::vector<uint8_t> request(size);
    if (size && std::fread(request.data(), 1, size, stdin) != size) return 1;
    Record record;
    std::vector<uint8_t> response;
    if (!Decode(request, &record) || !Generate(record, &response)) {
      const std::string error = "invalid record";
      response.assign(error.begin(), error.end());
    }
    uint8_t length[4] = {static_cast<uint8_t>(response.size() >> 24), static_cast<uint8_t>(response.size() >> 16),
                         static_cast<uint8_t>(response.size() >> 8), static_cast<uint8_t>(response.size())};
    if (std::fwrite(length, 1, 4, stdout) != 4 ||
        (!response.empty() && std::fwrite(response.data(), 1, response.size(), stdout) != response.size())) return 1;
    std::fflush(stdout);
  }
}
