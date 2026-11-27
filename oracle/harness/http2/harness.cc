#include <cstdint>
#include <iostream>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "base/command_line.h"
#include "base/containers/span.h"
#include "base/feature_list.h"
#include "base/functional/bind.h"
#include "base/memory/ref_counted.h"
#include "base/memory_coordinator/dummy_memory_consumer_registry.h"
#include "base/memory_coordinator/memory_consumer_registry.h"
#include "base/run_loop.h"
#include "base/test/task_environment.h"
#include "base/test/test_timeouts.h"
#include "net/base/host_port_pair.h"
#include "net/cert/cert_verifier.h"
#include "net/base/ip_endpoint.h"
#include "net/base/load_flags.h"
#include "net/base/load_timing_info.h"
#include "net/base/net_errors.h"
#include "net/base/network_anonymization_key.h"
#include "net/base/network_handle.h"
#include "net/base/request_priority.h"
#include "net/base/session_usage.h"
#include "net/dns/public/secure_dns_policy.h"
#include "net/http/http_network_session.h"
#include "net/http/http_server_properties.h"
#include "net/http/transport_security_state.h"
#include "net/quic/quic_context.h"
#include "net/log/net_log_with_source.h"
#include "net/base/proxy_chain.h"
#include "net/socket/next_proto.h"
#include "net/socket/socket_tag.h"
#include "net/socket/ssl_client_socket.h"
#include "net/ssl/ssl_config_service_defaults.h"
#include "net/ssl/ssl_connection_status_flags.h"
#include "net/socket/transport_client_socket.h"
#include "net/spdy/spdy_session.h"
#include "net/spdy/spdy_stream.h"
#include "net/traffic_annotation/network_traffic_annotation_test_helper.h"
#include "quiche/common/http/http_header_block.h"
#include "url/gurl.h"

namespace {

constexpr size_t kMaxRequests = 8;

struct Request {
  std::string method;
  std::string scheme;
  std::string host;
  std::string path;
  std::vector<std::pair<std::string, std::string>> headers;
  int priority = 2;
  uint16_t status = 200;
  bool reset = false;
  uint32_t error = 8;
  bool trailers = false;
};

struct Record {
  std::vector<Request> requests;
  bool reset_connection = false;
  std::string server_frames;
};

class Reader {
 public:
  explicit Reader(std::string_view data) : data_(data) {}
  bool U8(uint8_t* out) {
    if (data_.size() < 1) return false;
    *out = static_cast<uint8_t>(data_[0]);
    data_.remove_prefix(1);
    return true;
  }
  bool U16(uint32_t* out) {
    if (data_.size() < 2) return false;
    *out = (static_cast<uint32_t>(static_cast<uint8_t>(data_[0])) << 8) | static_cast<uint8_t>(data_[1]);
    data_.remove_prefix(2);
    return true;
  }
  bool U32(uint32_t* out) {
    if (data_.size() < 4) return false;
    *out = (static_cast<uint32_t>(static_cast<uint8_t>(data_[0])) << 24) |
           (static_cast<uint32_t>(static_cast<uint8_t>(data_[1])) << 16) |
           (static_cast<uint32_t>(static_cast<uint8_t>(data_[2])) << 8) | static_cast<uint8_t>(data_[3]);
    data_.remove_prefix(4);
    return true;
  }
  bool Short(std::string* out) {
    uint32_t size;
    return U16(&size) && Bytes(size, out);
  }
  bool Long(std::string* out) {
    uint32_t size;
    return U32(&size) && Bytes(size, out);
  }
  bool Bytes(uint32_t size, std::string* out) {
    if (data_.size() < size) return false;
    out->assign(data_.substr(0, size));
    data_.remove_prefix(size);
    return true;
  }
  bool Empty() const { return data_.empty(); }

 private:
  std::string_view data_;
};

std::optional<Record> Decode(std::string_view input) {
  Reader reader(input);
  Record record;
  uint8_t version, count;
  if (!reader.U8(&version) || version != 1) return std::nullopt;
  if (!reader.U8(&count) || count < 1 || count > kMaxRequests) return std::nullopt;
  for (uint32_t index = 0; index < count; ++index) {
    Request request;
    uint32_t header_count, status, error;
    uint8_t priority, reset, flags;
    if (!reader.Short(&request.method) || !reader.Short(&request.scheme) || !reader.Short(&request.host) ||
        !reader.Short(&request.path) || !reader.U16(&header_count)) {
      return std::nullopt;
    }
    for (uint32_t i = 0; i < header_count; ++i) {
      std::string name, value;
      if (!reader.Short(&name) || !reader.Short(&value)) return std::nullopt;
      request.headers.emplace_back(std::move(name), std::move(value));
    }
    if (!reader.U8(&priority) || priority > 5 || !reader.U16(&status) || status < 100 || status > 599 ||
        !reader.U8(&reset) || reset > 1 || !reader.U8(&flags) || flags > 1) {
      return std::nullopt;
    }
    request.priority = priority;
    request.status = static_cast<uint16_t>(status);
    request.reset = reset != 0;
    request.trailers = flags != 0;
    if (request.reset && !reader.U32(&error)) return std::nullopt;
    request.error = error;
    record.requests.push_back(std::move(request));
  }
  uint32_t settings_count, window_update;
  if (!reader.U16(&settings_count)) return std::nullopt;
  for (uint32_t i = 0; i < settings_count; ++i) {
    uint32_t identifier, value;
    if (!reader.U16(&identifier) || !reader.U32(&value)) return std::nullopt;
  }
  if (!reader.U32(&window_update)) return std::nullopt;
  uint8_t end;
  if (!reader.U8(&end) || end > 1 || !reader.Long(&record.server_frames) || !reader.Empty()) return std::nullopt;
  record.reset_connection = end == 1;
  return record;
}

class ScriptedSocket : public net::TransportClientSocket {
 public:
  ScriptedSocket(const Record& record, std::string* sent) : record_(&record), sent_(sent) {}
  ~ScriptedSocket() override = default;

  int Bind(const net::IPEndPoint&) override { return net::OK; }
  int Connect(net::CompletionOnceCallback) override { return net::OK; }
  void Disconnect() override {}
  bool IsConnected() const override { return true; }
  bool IsConnectedAndIdle() const override {
    return false;
  }
  int GetPeerAddress(net::IPEndPoint* address) const override {
    *address = net::IPEndPoint(net::IPAddress(127, 0, 0, 1), 443);
    return net::OK;
  }
  int GetLocalAddress(net::IPEndPoint* address) const override {
    *address = net::IPEndPoint(net::IPAddress(127, 0, 0, 1), 40000);
    return net::OK;
  }
  const net::NetLogWithSource& NetLog() const override { return net_log_; }
  bool WasEverUsed() const override { return true; }
  net::NextProto GetNegotiatedProtocol() const override {
    return net::NextProto::kProtoHTTP2;
  }
  bool GetSSLInfo(net::SSLInfo* info) override {
    net::SSLConnectionStatusSetVersion(net::SSL_CONNECTION_VERSION_TLS1_3, &info->connection_status);
    net::SSLConnectionStatusSetCipherSuite(0x1301, &info->connection_status);
    return true;
  }
  int64_t GetTotalReceivedBytes() const override { return received_; }
  void ApplySocketTag(const net::SocketTag&) override {}
  int SetReceiveBufferSize(int32_t) override { return net::OK; }
  int SetSendBufferSize(int32_t) override { return net::OK; }

  int ReadIfReady(net::IOBuffer* buf, int buf_len, net::CompletionOnceCallback callback) override {
    return Read(buf, buf_len, std::move(callback));
  }
  int Read(net::IOBuffer* buf, int buf_len, net::CompletionOnceCallback) override {
    const std::string& frames = record_->server_frames;
    if (offset_ >= frames.size()) {
      return record_->reset_connection ? net::ERR_CONNECTION_RESET : 0;
    }
    size_t size = std::min<size_t>(buf_len, frames.size() - offset_);
    buf->span().copy_prefix_from(base::as_byte_span(frames).subspan(offset_, size));
    offset_ += size;
    received_ += size;
    return static_cast<int>(size);
  }

  int Write(net::IOBuffer* buf, int buf_len, net::CompletionOnceCallback, const net::NetworkTrafficAnnotationTag&) override {
    sent_->append(base::as_string_view(buf->first(static_cast<size_t>(buf_len))));
    return buf_len;
  }

 private:
  raw_ptr<const Record> record_;
  raw_ptr<std::string> sent_;
  net::NetLogWithSource net_log_;
  size_t offset_ = 0;
  int64_t received_ = 0;
};

struct Events {
  std::vector<std::string> lines;
};

class Delegate : public net::SpdyStream::Delegate {
 public:
  Delegate(Events* events, spdy::SpdyStreamId id) : events_(events), id_(id) {}
  void OnHeadersSent() override {}
  void OnEarlyHintsReceived(const quiche::HttpHeaderBlock&) override {}
  void OnHeadersReceived(const quiche::HttpHeaderBlock& headers) override {
    auto status = headers.find(":status");
    events_->lines.push_back("headers " + std::to_string(id_) + " " +
                             (status == headers.end() ? std::string("-") : std::string(status->second)));
  }
  void OnDataReceived(std::unique_ptr<net::SpdyBuffer>) override {}
  void OnDataSent() override {}
  void OnTrailers(const quiche::HttpHeaderBlock&) override {
    events_->lines.push_back("trailers " + std::to_string(id_));
  }
  void OnClose(int status) override {
    events_->lines.push_back("close " + std::to_string(id_) + " " + net::ErrorToShortString(status));
  }
  bool CanGreaseFrameType() const override { return false; }
  net::NetLogSource source_dependency() const override { return net::NetLogSource(); }

 private:
  raw_ptr<Events> events_;
  spdy::SpdyStreamId id_;
};

std::string Hex(std::string_view data) {
  static constexpr char digits[] = "0123456789abcdef";
  std::string result;
  result.reserve(data.size() * 2);
  for (unsigned char byte : data) {
    result += digits[byte >> 4];
    result += digits[byte & 15];
  }
  return result;
}

class RejectingCertVerifier : public net::CertVerifier {
 public:
  int Verify(const RequestParams&, net::CertVerifyResult*, net::CompletionOnceCallback, std::unique_ptr<Request>*,
             const net::NetLogWithSource&) override {
    return net::ERR_CERT_INVALID;
  }
  void Verify2QwacBinding(const std::string&, const std::string&, const scoped_refptr<net::X509Certificate>&,
                          base::OnceCallback<void(const scoped_refptr<net::X509Certificate>&)> callback,
                          const net::NetLogWithSource&) override {
    std::move(callback).Run(nullptr);
  }
  void SetConfig(const Config&) override {}
  void AddObserver(Observer*) override {}
  void RemoveObserver(Observer*) override {}
};

std::string Execute(std::string_view input) {
  std::optional<Record> decoded = Decode(input);
  if (!decoded) return "invalid record\n";
  const Record& record = *decoded;
  std::string sent;
  Events events;
  {
    net::HttpNetworkSessionParams params;
    params.enable_http2 = true;
    params.enable_quic = false;
    params.enable_spdy_ping_based_connection_checking = false;
    params.enable_http2_settings_grease = false;
    params.http2_end_stream_with_data_frame = false;
    net::QuicContext quic_context;
    net::HttpServerProperties http_server_properties;
    net::TransportSecurityState transport_security_state;
    RejectingCertVerifier cert_verifier;
    net::SSLConfigServiceDefaults ssl_config_service;
    net::HttpNetworkSessionContext context;
    context.quic_context = &quic_context;
    context.http_server_properties = &http_server_properties;
    context.transport_security_state = &transport_security_state;
    context.cert_verifier = &cert_verifier;
    context.ssl_config_service = &ssl_config_service;
    context.net_log = net::NetLog::Get();
    net::HttpNetworkSession session(params, context);
    net::SpdySessionKey key(net::HostPortPair("verified.https.test", 443), net::PRIVACY_MODE_DISABLED,
                            net::ProxyChain::Direct(), net::SessionUsage::kDestination, net::SocketTag(),
                            net::NetworkAnonymizationKey(), net::SecureDnsPolicy::kAllow,
                            /*disable_cert_verification_network_fetches=*/false, net::handles::kInvalidNetworkHandle);
    auto socket = std::make_unique<ScriptedSocket>(record, &sent);
    net::LoadTimingInfo::ConnectTiming timing;
    net::NetLogWithSource net_log;
    auto result = session.spdy_session_pool()->CreateAvailableSessionFromSocket(
        key, std::move(socket), timing, net_log, net::SpdySessionInitiator::kUnknown);
    if (!result.has_value()) return "session " + std::string(net::ErrorToShortString(result.error())) + "\n";
    base::WeakPtr<net::SpdySession> spdy_session = result.value();
    std::vector<std::unique_ptr<Delegate>> delegates;
    std::vector<std::unique_ptr<net::SpdyStreamRequest>> requests;
    for (const Request& item : record.requests) {
      auto request = std::make_unique<net::SpdyStreamRequest>();
      int rv = request->StartRequest(net::SPDY_REQUEST_RESPONSE_STREAM, spdy_session,
                                     GURL(item.scheme + "://" + item.host + item.path),
                                     /*can_send_early=*/false, static_cast<net::RequestPriority>(item.priority),
                                     net::SocketTag(), net_log, net::CompletionOnceCallback(),
                                     TRAFFIC_ANNOTATION_FOR_TESTS);
      if (rv != net::OK) {
        events.lines.push_back("start " + std::string(net::ErrorToShortString(rv)));
        continue;
      }
      auto stream = request->ReleaseStream();
      delegates.push_back(std::make_unique<Delegate>(&events, stream->stream_id()));
      stream->SetDelegate(delegates.back().get());
      quiche::HttpHeaderBlock headers;
      headers[":method"] = item.method;
      headers[":authority"] = item.host;
      headers[":scheme"] = item.scheme;
      headers[":path"] = item.path;
      for (const auto& [name, value] : item.headers) headers[name] = value;
      stream->SendRequestHeaders(std::move(headers), net::NO_MORE_DATA_TO_SEND);
      requests.push_back(std::move(request));
    }
    base::RunLoop loop;
    loop.RunUntilIdle();
  }
  std::string output = "wire " + Hex(sent) + "\n";
  for (const std::string& line : events.lines) output += line + "\n";
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
  base::ScopedMemoryConsumerRegistry<base::DummyMemoryConsumerRegistry> memory_consumers;
  base::test::TaskEnvironment environment(base::test::TaskEnvironment::MainThreadType::IO,
                                          base::test::TaskEnvironment::TimeSource::SYSTEM_TIME,
                                          base::test::TaskEnvironment::ThreadPoolExecutionMode::QUEUED);
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
