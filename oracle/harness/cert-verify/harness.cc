#include <algorithm>
#include <cstdint>
#include <iostream>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "base/at_exit.h"
#include "base/command_line.h"
#include "base/feature_list.h"
#include "base/memory_coordinator/dummy_memory_consumer_registry.h"
#include "base/memory_coordinator/memory_consumer_registry.h"
#include "base/strings/string_number_conversions.h"
#include "base/strings/string_util.h"
#include "base/time/time.h"
#include "base/time/time_override.h"
#include "base/base64.h"
#include "components/certificate_transparency/chrome_ct_policy_enforcer.h"
#include "components/certificate_transparency/chrome_require_ct_delegate.h"
#include "components/certificate_transparency/ct_known_logs.h"
#include "components/version_info/version_info.h"
#include "crypto/hash.h"
#include "net/base/features.h"
#include "net/base/ip_address.h"
#include "net/base/net_errors.h"
#include "net/cert/cert_net_fetcher.h"
#include "net/cert/cert_status_flags.h"
#include "net/cert/cert_verify_proc.h"
#include "net/cert/cert_verify_proc_builtin.h"
#include "net/cert/cert_verify_result.h"
#include "net/cert/crl_set.h"
#include "net/cert/ct_log_verifier.h"
#include "net/cert/ct_policy_status.h"
#include "net/cert/internal/chrome_root_store_data.h"
#include "net/cert/internal/system_trust_store.h"
#include "net/cert/internal/trust_store_chrome.h"
#include "net/cert/multi_log_ct_verifier.h"
#include "net/cert/x509_certificate.h"
#include "net/cert/x509_util.h"
#include "net/log/net_log_with_source.h"
#include "third_party/boringssl/src/include/openssl/pool.h"
#include "third_party/boringssl/src/pki/parsed_certificate.h"

namespace {

base::Time g_now;

base::Time FixedNow() {
  return g_now;
}

enum class TrustKind : uint8_t {
  kAnchor = 0,
  kAnchorWithEnforcedConstraints = 1,
  kAnchorWithConstraints = 2,
  kLeaf = 3,
  kAnchorOrLeaf = 4,
  kUntrustedAuthority = 5,
  kDistrustedSpki = 6,
};

struct Cidr {
  std::string ip;
  std::string mask;
};

struct TrustEntry {
  TrustKind kind;
  std::string der;
  std::vector<std::string> dns_names;
  std::vector<Cidr> cidrs;
};

struct Request {
  int64_t time = 0;
  std::string hostname;
  std::vector<std::string> chain;
  std::vector<TrustEntry> trust;
  std::string sct_list;
  std::string ocsp_response;
};

class Reader {
 public:
  explicit Reader(std::string_view data) : data_(data) {}
  bool Bytes(size_t size, std::string* out) {
    if (data_.size() < size) return false;
    out->assign(data_.substr(0, size));
    data_.remove_prefix(size);
    return true;
  }
  bool Uint(size_t width, uint64_t* out) {
    if (data_.size() < width) return false;
    uint64_t value = 0;
    for (size_t i = 0; i < width; ++i) value = (value << 8) | static_cast<uint8_t>(data_[i]);
    data_.remove_prefix(width);
    *out = value;
    return true;
  }
  bool Prefixed(size_t width, std::string* out) {
    uint64_t size;
    return Uint(width, &size) && Bytes(size, out);
  }
  bool empty() const { return data_.empty(); }

 private:
  std::string_view data_;
};

std::optional<std::string> Parse(std::string_view input, Request* request) {
  Reader reader(input);
  uint64_t value;
  if (!reader.Uint(1, &value) || value != 1) return "version";
  if (!reader.Uint(8, &value)) return "time";
  request->time = static_cast<int64_t>(value);
  if (request->time < -62135596800 || request->time > 253402300799) return "time";
  if (!reader.Prefixed(2, &request->hostname) || request->hostname.empty()) return "hostname";
  if (!reader.Uint(2, &value) || value == 0) return "chain";
  for (uint64_t i = 0; i < value; ++i) {
    std::string der;
    if (!reader.Prefixed(4, &der) || der.empty()) return "chain";
    request->chain.push_back(std::move(der));
  }
  uint64_t count;
  if (!reader.Uint(2, &count)) return "trust";
  for (uint64_t i = 0; i < count; ++i) {
    TrustEntry entry;
    if (!reader.Uint(1, &value) || value > 6) return "trust kind";
    entry.kind = static_cast<TrustKind>(value);
    if (!reader.Prefixed(4, &entry.der) || entry.der.empty()) return "trust certificate";
    if (entry.kind == TrustKind::kAnchorWithConstraints || entry.kind == TrustKind::kLeaf ||
        entry.kind == TrustKind::kAnchorOrLeaf) {
      uint64_t names;
      if (!reader.Uint(2, &names)) return "trust names";
      for (uint64_t j = 0; j < names; ++j) {
        std::string name;
        if (!reader.Prefixed(2, &name)) return "trust names";
        entry.dns_names.push_back(std::move(name));
      }
      uint64_t cidrs;
      if (!reader.Uint(2, &cidrs)) return "trust cidrs";
      for (uint64_t j = 0; j < cidrs; ++j) {
        uint64_t width;
        Cidr cidr;
        if (!reader.Uint(1, &width) || (width != 4 && width != 16) || !reader.Bytes(width, &cidr.ip) ||
            !reader.Bytes(width, &cidr.mask)) {
          return "trust cidrs";
        }
        entry.cidrs.push_back(std::move(cidr));
      }
    }
    request->trust.push_back(std::move(entry));
  }
  if (!reader.empty()) {
    if (!reader.Prefixed(2, &request->sct_list)) return "sct list";
    if (!reader.empty() && !reader.Prefixed(2, &request->ocsp_response)) return "ocsp response";
  }
  if (!reader.empty()) return "trailing bytes";
  return std::nullopt;
}

std::vector<uint8_t> Bytes(const std::string& data) {
  return std::vector<uint8_t>(data.begin(), data.end());
}

net::IPAddress Address(const std::string& data) {
  return net::IPAddress(base::as_byte_span(data));
}

std::shared_ptr<const bssl::ParsedCertificate> ParseDirect(const std::string& der) {
  return bssl::ParsedCertificate::Create(net::x509_util::CreateCryptoBuffer(base::as_byte_span(der)),
                                         net::x509_util::DefaultParseCertificateOptions(), nullptr);
}

net::CertVerifyProc::InstanceParams BuildInstanceParams(const std::vector<TrustEntry>& trust) {
  std::vector<std::vector<uint8_t>> anchors;
  std::vector<std::vector<uint8_t>> enforced;
  std::vector<std::vector<uint8_t>> authorities;
  net::CertVerifyProc::InstanceParams params;
  for (const TrustEntry& entry : trust) {
    switch (entry.kind) {
      case TrustKind::kAnchor:
        anchors.push_back(Bytes(entry.der));
        break;
      case TrustKind::kAnchorWithEnforcedConstraints:
        enforced.push_back(Bytes(entry.der));
        break;
      case TrustKind::kUntrustedAuthority:
        authorities.push_back(Bytes(entry.der));
        break;
      case TrustKind::kDistrustedSpki:
        params.additional_distrusted_spkis.push_back(Bytes(entry.der));
        break;
      case TrustKind::kAnchorWithConstraints:
      case TrustKind::kLeaf:
      case TrustKind::kAnchorOrLeaf: {
        std::shared_ptr<const bssl::ParsedCertificate> cert = ParseDirect(entry.der);
        if (!cert) break;
        net::CertVerifyProc::CertificateWithConstraints constrained;
        constrained.certificate = std::move(cert);
        constrained.permitted_dns_names = entry.dns_names;
        for (const Cidr& cidr : entry.cidrs) constrained.permitted_cidrs.push_back({Address(cidr.ip), Address(cidr.mask)});
        if (entry.kind == TrustKind::kAnchorWithConstraints) {
          params.additional_trust_anchors_with_constraints.push_back(std::move(constrained));
        } else if (entry.kind == TrustKind::kLeaf) {
          params.additional_trust_leafs.push_back(std::move(constrained));
        } else {
          params.additional_trust_anchors_and_leafs.push_back(std::move(constrained));
        }
        break;
      }
    }
  }
  params.additional_trust_anchors =
      net::x509_util::ParseAllValidCerts(net::x509_util::ConvertToX509CertificatesIgnoreErrors(anchors));
  params.additional_trust_anchors_with_enforced_constraints =
      net::x509_util::ParseAllValidCerts(net::x509_util::ConvertToX509CertificatesIgnoreErrors(enforced));
  params.additional_untrusted_authorities =
      net::x509_util::ParseAllValidCerts(net::x509_util::ConvertToX509CertificatesIgnoreErrors(authorities));
  params.require_ct_delegate = base::MakeRefCounted<certificate_transparency::ChromeRequireCTDelegate>();
  return params;
}

struct CtConfiguration {
  std::vector<scoped_refptr<const net::CTLogVerifier>> logs;
  std::vector<std::pair<std::string, base::Time>> disqualified;
  std::map<std::string, certificate_transparency::LogInfo> info;
};

network::mojom::CTLogInfo::LogType MojoLogType(certificate_transparency::LogType type) {
  switch (type) {
    case certificate_transparency::LogType::kUnspecified:
      return network::mojom::CTLogInfo::LogType::kUnspecified;
    case certificate_transparency::LogType::kRFC6962:
      return network::mojom::CTLogInfo::LogType::kRFC6962;
    case certificate_transparency::LogType::kStaticCTAPI:
      return network::mojom::CTLogInfo::LogType::kStaticCTAPI;
  }
}

CtConfiguration StaticCtConfiguration() {
  CtConfiguration config;
  std::vector<std::pair<std::string, base::Time>> disqualified = certificate_transparency::GetDisqualifiedLogs();
  for (const auto& log : certificate_transparency::GetKnownLogs()) {
    std::string key(log.log_key, log.log_key_length);
    std::string id(base::as_string_view(crypto::hash::Sha256(key)));
    scoped_refptr<const net::CTLogVerifier> verifier = net::CTLogVerifier::Create(key, log.log_name);
    if (verifier) config.logs.push_back(std::move(verifier));
    for (const auto& [log_id, when] : disqualified) {
      if (log_id == id) config.disqualified.emplace_back(id, when);
    }
    certificate_transparency::OperatorHistoryEntry history;
    history.current_operator = log.current_operator;
    for (size_t i = 0; i < log.previous_operators_length; ++i) {
      history.previous_operators.emplace_back(log.previous_operators[i].name, log.previous_operators[i].end_time);
    }
    certificate_transparency::LogInfo info;
    info.operator_history = history;
    info.log_type = MojoLogType(log.log_type);
    config.info[id] = info;
  }
  std::sort(config.disqualified.begin(), config.disqualified.end());
  return config;
}

const char* ComplianceName(net::ct::CTPolicyCompliance compliance) {
  switch (compliance) {
    case net::ct::CTPolicyCompliance::CT_POLICY_COMPLIES_VIA_SCTS:
      return "complies";
    case net::ct::CTPolicyCompliance::CT_POLICY_NOT_ENOUGH_SCTS:
      return "not-enough-scts";
    case net::ct::CTPolicyCompliance::CT_POLICY_NOT_DIVERSE_SCTS:
      return "not-diverse-scts";
    case net::ct::CTPolicyCompliance::CT_POLICY_BUILD_NOT_TIMELY:
      return "build-not-timely";
    case net::ct::CTPolicyCompliance::CT_POLICY_COMPLIANCE_DETAILS_NOT_AVAILABLE:
      return "details-not-available";
    case net::ct::CTPolicyCompliance::CT_POLICY_COUNT:
      break;
  }
  return "invalid";
}

std::string StatusNames(net::CertStatus status) {
  static constexpr std::pair<net::CertStatus, const char*> kFlags[] = {
#define CERT_STATUS_FLAG(label, value) {value, #label},
#include "net/cert/cert_status_flags_list.h"
#undef CERT_STATUS_FLAG
  };
  std::string names;
  net::CertStatus seen = 0;
  for (const auto& [flag, name] : kFlags) {
    if (status & flag) {
      names += std::string(names.empty() ? "" : "|") + name;
      seen |= flag;
    }
  }
  if (status & ~seen) names += std::string(names.empty() ? "" : "|") + "0x" + base::HexEncode(base::byte_span_from_ref(status));
  return names.empty() ? "none" : names;
}

std::string Execute(std::string_view input) {
  Request request;
  if (std::optional<std::string> error = Parse(input, &request)) return "invalid " + *error + "\n";

  std::vector<bssl::UniquePtr<CRYPTO_BUFFER>> intermediates;
  for (size_t i = 1; i < request.chain.size(); ++i) {
    intermediates.push_back(net::x509_util::CreateCryptoBuffer(base::as_byte_span(request.chain[i])));
  }
  scoped_refptr<net::X509Certificate> cert = net::X509Certificate::CreateFromBuffer(
      net::x509_util::CreateCryptoBuffer(base::as_byte_span(request.chain[0])), std::move(intermediates));
  if (!cert) {
    return "error " + base::NumberToString(net::ERR_SSL_SERVER_CERT_BAD_FORMAT) + " " +
           net::ErrorToShortString(net::ERR_SSL_SERVER_CERT_BAD_FORMAT) + "\n";
  }

  g_now = base::Time::UnixEpoch() + base::Seconds(request.time);
  base::subtle::ScopedTimeClockOverrides clock(&FixedNow, nullptr, nullptr);

  CtConfiguration ct = StaticCtConfiguration();
  auto enforcer = base::MakeRefCounted<certificate_transparency::ChromeCTPolicyEnforcer>(
      certificate_transparency::GetLogListTimestamp(), std::move(ct.disqualified), std::move(ct.info),
      base::FeatureList::IsEnabled(net::features::kEnforceOneRfc6962CtPolicy));
  scoped_refptr<net::CertVerifyProc> proc = net::CreateCertVerifyProcBuiltin(
      nullptr, net::CRLSet::BuiltinCRLSet(), std::make_unique<net::MultiLogCTVerifier>(ct.logs), std::move(enforcer),
      net::CreateChromeOnlySystemTrustStore(std::make_unique<net::TrustStoreChrome>()),
      BuildInstanceParams(request.trust), std::nullopt);

  net::CertVerifyResult result;
  int rv = proc->Verify(cert.get(), request.hostname, request.ocsp_response, request.sct_list, 0, &result,
                        net::NetLogWithSource());

  std::string output = "error " + base::NumberToString(rv) + " " + net::ErrorToShortString(rv) + "\n";
  output += "status " + StatusNames(result.cert_status) + "\n";
  output += std::string("ct ") + ComplianceName(result.policy_compliance) + "\n";
  output += "chain";
  for (const auto& buffer : result.verified_cert->cert_buffers()) {
    output += " " + base::ToLowerASCII(base::HexEncode(crypto::hash::Sha256(net::x509_util::CryptoBufferAsSpan(buffer.get()))));
  }
  output += "\n";
  return output;
}

namespace blocklist {
#include "net/cert/cert_verify_proc_blocklist.inc"
}

std::string Micros(const std::optional<base::Time>& time) {
  if (!time) return "-";
  return base::NumberToString((*time - base::Time::UnixEpoch()).InMicroseconds());
}

std::string Hex(base::span<const uint8_t> data) {
  return base::ToLowerASCII(base::HexEncode(data));
}

std::string DumpTrustData() {
  std::string out = "chrome-version " + std::string(version_info::GetVersionNumber()) + "\n";
  net::ChromeRootStoreData data = net::ChromeRootStoreData::CreateFromCompiledRootStore();
  out += "crs-version " + base::NumberToString(data.version()) + "\n";
  for (const auto& anchor : data.trust_anchors()) {
    out += "anchor " + base::Base64Encode(anchor.certificate->der_cert()) + " expiry " +
           (anchor.enforce_anchor_expiry ? "1" : "0") + " constraints " + (anchor.enforce_anchor_constraints ? "1" : "0") +
           " id " + (anchor.crs_root_id ? base::NumberToString(*anchor.crs_root_id) : std::string("-")) + "\n";
    for (const auto& constraint : anchor.constraints) {
      std::string dns;
      for (const auto& name : constraint.permitted_dns_names) dns += (dns.empty() ? "" : ",") + name;
      out += "constraint sct-not-after " + Micros(constraint.sct_not_after) + " sct-all-after " +
             Micros(constraint.sct_all_after) + " min-version " +
             (constraint.min_version ? constraint.min_version->GetString() : std::string("-")) +
             " max-version-exclusive " +
             (constraint.max_version_exclusive ? constraint.max_version_exclusive->GetString() : std::string("-")) +
             " dns " + (dns.empty() ? std::string("-") : dns) + " serial-not-after " +
             (constraint.serial_not_after ? base::NumberToString(*constraint.serial_not_after) : std::string("-")) +
             " serial-after " +
             (constraint.serial_after ? base::NumberToString(*constraint.serial_after) : std::string("-")) +
             " validity-starts-not-after " + Micros(constraint.validity_starts_not_after) +
             " validity-starts-after " + Micros(constraint.validity_starts_after) + "\n";
    }
  }
  out += "ct-log-list " + Micros(certificate_transparency::GetLogListTimestamp()) + "\n";
  std::map<std::string, int64_t> disqualified;
  for (const auto& [id, when] : certificate_transparency::GetDisqualifiedLogs()) {
    disqualified[id] = (when - base::Time::UnixEpoch()).InMicroseconds();
  }
  for (const auto& log : certificate_transparency::GetKnownLogs()) {
    std::string key(log.log_key, log.log_key_length);
    std::string id(base::as_string_view(crypto::hash::Sha256(key)));
    const char* type = log.log_type == certificate_transparency::LogType::kRFC6962 ? "rfc6962"
                       : log.log_type == certificate_transparency::LogType::kStaticCTAPI ? "static"
                                                                                       : "unspecified";
    std::string previous;
    for (size_t i = 0; i < log.previous_operators_length; ++i) {
      previous += (previous.empty() ? "" : ",") + base::Base64Encode(log.previous_operators[i].name) + ":" +
                  Micros(log.previous_operators[i].end_time);
    }
    auto found = disqualified.find(id);
    out += "ct-log " + base::ToLowerASCII(base::HexEncode(base::as_byte_span(id))) + " " + base::Base64Encode(key) + " " + type +
           " " + base::Base64Encode(log.current_operator) + " " + (previous.empty() ? "-" : previous) + " " +
           (found == disqualified.end() ? std::string("-") : base::NumberToString(found->second)) + "\n";
  }
  for (const auto& ev : net::GetEvRootCaMetadata()) {
    for (const auto& oid : ev.policy_oids) {
      if (!oid.empty()) out += "ev " + Hex(ev.fingerprint) + " " + std::string(oid) + "\n";
    }
  }
  for (const auto& hash : blocklist::kSPKIBlockList) out += "blocked-spki " + Hex(hash) + "\n";
  for (const auto& hash : blocklist::kKnownInterceptionList) out += "interception-spki " + Hex(hash) + "\n";
  return out;
}

bool ReadExact(char* data, size_t size) {
  std::cin.read(data, static_cast<std::streamsize>(size));
  return static_cast<size_t>(std::cin.gcount()) == size;
}

}  // namespace

int main(int argc, char** argv) {
  base::AtExitManager exit_manager;
  base::CommandLine::Init(argc, argv);
  base::FeatureList::InitInstance("", "");
  base::ScopedMemoryConsumerRegistry<base::DummyMemoryConsumerRegistry> memory_consumers;
  if (argc == 2 && std::string_view(argv[1]) == "--dump-trust-data") {
    std::cout << DumpTrustData();
    return 0;
  }
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
    std::string output = Execute(input);
    uint32_t length = output.size();
    char header[4] = {static_cast<char>(length >> 24), static_cast<char>(length >> 16), static_cast<char>(length >> 8),
                      static_cast<char>(length)};
    std::cout.write(header, 4).write(output.data(), output.size()).flush();
  }
}
