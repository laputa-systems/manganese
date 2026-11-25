#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <unistd.h>

#include "net/base/registry_controlled_domains/registry_controlled_domain.h"
#include "net/base/schemeful_site.h"
#include "third_party/blink/renderer/platform/weborigin/kurl.h"
#include "third_party/blink/renderer/platform/wtf/allocator/partitions.h"
#include "third_party/blink/renderer/platform/wtf/text/wtf_string.h"
#include "third_party/blink/renderer/platform/wtf/wtf.h"
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

std::optional<GURL> set_component(const GURL& original, std::string_view name,
                                  std::string_view value, bool direct) {
  blink::KURL url(blink::String::FromUtf8(original.spec()));
  blink::String argument = blink::String::FromUtf8(value);
  if (name == "href") {
    blink::KURL result(argument);
    if (!result.IsValid()) return direct ? std::optional<GURL>(original) : std::nullopt;
    return direct ? static_cast<GURL>(result) : GURL(result.GetString().Utf8().data());
  }
  if (name == "protocol") url.SetProtocol(argument);
  else if (name == "username") url.SetUser(argument);
  else if (name == "password") url.SetPass(argument);
  else if (name == "host" || name == "hostname") {
    if ((!argument.empty() || url.CanRemoveHost()) && url.CanSetHostOrPort()) {
      if (name == "host") url.SetHostAndPort(argument);
      else url.SetHost(argument);
    }
  } else if (name == "port") {
    if (url.CanSetHostOrPort()) {
      if (argument.empty()) url.RemovePort();
      else url.SetPort(argument);
    }
  } else if (name == "pathname") {
    if (url.CanSetPathname()) url.SetPath(argument);
  } else if (name == "search") {
    if (argument.empty() || argument == "?") url.SetQuery(blink::String());
    else url.SetQuery(argument);
  } else if (name == "hash") {
    if (argument.empty()) url.RemoveFragmentIdentifier();
    else if (argument.starts_with('#')) url.SetFragmentIdentifier(argument.substr(1));
    else url.SetFragmentIdentifier(argument);
  } else return std::nullopt;
  if (!url.IsValid() && (!direct || name != "protocol")) return original;
  return direct ? static_cast<GURL>(url) : GURL(url.GetString().Utf8().data());
}

std::optional<std::string> respond(std::string_view request) {
  Field input, base, other;
  if (!take(request, input) || !input || !take(request, base) ||
      !take(request, other) || request.empty() || request[0] > 1)
    return std::nullopt;
  bool include_private = request[0];
  request.remove_prefix(1);
  Field setter, setter_value;
  if (!request.empty() && (!take(request, setter) || !setter || !take(request, setter_value) || !setter_value || !request.empty()))
    return std::nullopt;
  GURL value = base ? GURL(*base).Resolve(*input) : GURL(*input);
  if (!value.is_valid()) return std::string(1, '\0');
  bool schemeful = setter && *setter == "schemeful-site";
  if (setter && !schemeful) {
    bool direct = setter->starts_with("blink:");
    auto name = direct ? setter->substr(6) : *setter;
    auto updated = set_component(value, name, *setter_value, direct);
    if (!updated) return std::string(1, '\0');
    value = std::move(*updated);
    if (!value.is_valid()) return std::string(1, '\0');
  }
  auto policy = include_private ? net::registry_controlled_domains::INCLUDE_PRIVATE_REGISTRIES
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
  if (schemeful && other) {
    out.push_back(net::SchemefulSite::IsSameSite(value, GURL(*other)) ? 1 : 0);
  } else if (!other || !value.has_host() || !GURL(*other).is_valid() || !GURL(*other).has_host()) {
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
  blink::Partitions::Initialize();
  blink::InitializeWtf();
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
