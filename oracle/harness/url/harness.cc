#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <unistd.h>

#include "net/base/registry_controlled_domains/registry_controlled_domain.h"
#include "third_party/icu/source/common/unicode/udata.h"
#include "url/gurl.h"
#include "url/origin.h"

using Field = std::optional<std::string_view>;
extern "C" const unsigned char _binary_icudtl_dat_start[];

uint32_t number(std::string_view bytes) {
  return (static_cast<uint32_t>(static_cast<unsigned char>(bytes[0])) << 24) |
         (static_cast<uint32_t>(static_cast<unsigned char>(bytes[1])) << 16) |
         (static_cast<uint32_t>(static_cast<unsigned char>(bytes[2])) << 8) |
         static_cast<unsigned char>(bytes[3]);
}

void append_number(std::string& out, uint32_t value) {
  for (int shift = 24; shift >= 0; shift -= 8)
    out.push_back(static_cast<char>(value >> shift));
}

bool take(std::string_view& request, Field& field) {
  if (request.size() < 4) return false;
  uint32_t size = number(request);
  request.remove_prefix(4);
  if (size == UINT32_MAX) { field = std::nullopt; return true; }
  if (size > request.size()) return false;
  field = request.substr(0, size);
  request.remove_prefix(size);
  return true;
}

void append(std::string& out, Field field) {
  if (!field) { append_number(out, UINT32_MAX); return; }
  append_number(out, field->size());
  out.append(*field);
}

std::optional<std::string> respond(std::string_view request) {
  Field input, base, other;
  if (!take(request, input) || !input || !take(request, base) ||
      !take(request, other) || request.size() != 1 || request[0] > 1)
    return std::nullopt;
  GURL value = base ? GURL(*base).Resolve(*input) : GURL(*input);
  if (!value.is_valid()) return std::string(1, '\0');
  auto policy = request[0] ? net::registry_controlled_domains::INCLUDE_PRIVATE_REGISTRIES
                           : net::registry_controlled_domains::EXCLUDE_PRIVATE_REGISTRIES;
  auto origin = url::Origin::Create(value);
  std::string serialized = origin.Serialize();
  std::string domain = net::registry_controlled_domains::GetDomainAndRegistry(value, policy);
  std::string out(1, '\1');
  out.push_back(origin.opaque() ? 1 : 0);
  append(out, value.spec());
  append(out, value.scheme());
  append(out, value.username());
  append(out, value.has_password() ? Field(value.password()) : std::nullopt);
  append(out, value.has_host() ? Field(value.host()) : std::nullopt);
  append(out, value.has_port() ? Field(value.port()) : std::nullopt);
  append(out, value.path());
  append(out, value.has_query() ? Field(value.query()) : std::nullopt);
  append(out, value.has_ref() ? Field(value.ref()) : std::nullopt);
  append(out, serialized);
  append(out, domain.empty() ? std::nullopt : Field(domain));
  if (!other || !value.has_host() || !GURL(*other).is_valid() || !GURL(*other).has_host()) {
    out.push_back(2);
  } else {
    out.push_back(net::registry_controlled_domains::SameDomainOrHost(value, GURL(*other), policy) ? 1 : 0);
  }
  return out;
}

bool read_exact(char* data, size_t length) {
  while (length) {
    ssize_t n = read(STDIN_FILENO, data, length);
    if (n <= 0) return false;
    data += n;
    length -= n;
  }
  return true;
}

bool write_all(std::string_view data) {
  while (!data.empty()) {
    ssize_t n = write(STDOUT_FILENO, data.data(), data.size());
    if (n <= 0) return false;
    data.remove_prefix(n);
  }
  return true;
}

int main() {
  UErrorCode error = U_ZERO_ERROR;
  udata_setCommonData(_binary_icudtl_dat_start, &error);
  if (U_FAILURE(error)) return 1;
  char header[4];
  if (!read_exact(header, 4) || std::string_view(header, 4) != "MGB1" || !write_all("MGB1")) return 1;
  while (true) {
    ssize_t n = read(STDIN_FILENO, header, 1);
    if (n == 0) return 0;
    if (n != 1 || !read_exact(header + 1, 3)) return 1;
    std::string request(number(std::string_view(header, 4)), '\0');
    if (!read_exact(request.data(), request.size())) return 1;
    auto response = respond(request);
    if (!response) return 1;
    std::string length;
    append_number(length, response->size());
    if (!write_all(length) || !write_all(*response)) return 1;
  }
}
