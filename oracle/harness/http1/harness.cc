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
#include "base/memory/raw_ptr.h"
#include "base/memory_coordinator/dummy_memory_consumer_registry.h"
#include "base/memory_coordinator/memory_consumer_registry.h"
#include "base/run_loop.h"
#include "base/test/task_environment.h"
#include "base/test/test_timeouts.h"
#include "net/base/chunked_upload_data_stream.h"
#include "net/base/elements_upload_data_stream.h"
#include "net/base/io_buffer.h"
#include "net/base/ip_endpoint.h"
#include "net/base/load_flags.h"
#include "net/base/net_errors.h"
#include "net/base/upload_bytes_element_reader.h"
#include "net/cert/cert_verifier.h"
#include "net/cert/cert_verify_result.h"
#include "net/http/http_network_session.h"
#include "net/http/http_request_headers.h"
#include "net/http/http_request_info.h"
#include "net/http/http_response_headers.h"
#include "net/http/http_response_info.h"
#include "net/http/http_transaction.h"
#include "net/http/http_transaction_factory.h"
#include "net/http/http_util.h"
#include "net/log/net_log_with_source.h"
#include "net/proxy_resolution/configured_proxy_resolution_service.h"
#include "net/proxy_resolution/proxy_config.h"
#include "net/proxy_resolution/proxy_config_with_annotation.h"
#include "net/socket/client_socket_factory.h"
#include "net/socket/datagram_client_socket.h"
#include "net/socket/socket_performance_watcher.h"
#include "net/socket/ssl_client_socket.h"
#include "net/socket/transport_client_socket.h"
#include "net/traffic_annotation/network_traffic_annotation_test_helper.h"
#include "net/url_request/url_request_context.h"
#include "net/url_request/url_request_context_builder.h"
#include "url/gurl.h"

namespace {

constexpr int kBodyReadSize = 32 * 1024;
constexpr size_t kMaxSegment = 16 * 1024;

enum class End : uint8_t { kClose = 0, kReset = 1, kHang = 2 };

struct Request {
  std::string url;
  std::string method;
  int load_flags = 0;
  uint8_t body_kind = 0;
  std::vector<std::string> body;
  std::vector<std::pair<std::string, std::string>> headers;
  std::string referrer;
  std::string user_agent;
  std::string accept_language;
  std::string cookie_line;
  std::vector<std::string> segments;
  End end = End::kClose;
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
  bool Bytes(uint32_t size, std::string* out) {
    if (data_.size() < size) return false;
    out->assign(data_.substr(0, size));
    data_.remove_prefix(size);
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
  bool Empty() const { return data_.empty(); }

 private:
  std::string_view data_;
};

std::optional<Request> Decode(std::string_view input) {
  Reader reader(input);
  Request request;
  uint8_t version, flags, end;
  uint32_t count;
  if (!reader.U8(&version) || version != 1) return std::nullopt;
  if (!reader.Short(&request.url) || !reader.Short(&request.method) || !reader.U8(&flags) || flags > 2) return std::nullopt;
  request.load_flags = flags == 1 ? net::LOAD_BYPASS_CACHE : flags == 2 ? net::LOAD_VALIDATE_CACHE : 0;
  if (!reader.U8(&request.body_kind) || request.body_kind > 2) return std::nullopt;
  if (request.body_kind == 1) {
    request.body.emplace_back();
    if (!reader.Long(&request.body.back())) return std::nullopt;
  } else if (request.body_kind == 2) {
    if (!reader.U16(&count)) return std::nullopt;
    for (uint32_t i = 0; i < count; ++i) {
      request.body.emplace_back();
      if (!reader.Long(&request.body.back()) || request.body.back().empty()) return std::nullopt;
    }
  }
  if (!reader.U16(&count)) return std::nullopt;
  for (uint32_t i = 0; i < count; ++i) {
    std::string name, value;
    if (!reader.Short(&name) || !reader.Short(&value)) return std::nullopt;
    request.headers.emplace_back(std::move(name), std::move(value));
  }
  if (!reader.Short(&request.referrer) || !reader.Short(&request.user_agent) || !reader.Short(&request.accept_language) ||
      !reader.Short(&request.cookie_line) || !reader.U16(&count)) {
    return std::nullopt;
  }
  for (uint32_t i = 0; i < count; ++i) {
    request.segments.emplace_back();
    if (!reader.Long(&request.segments.back()) || request.segments.back().empty() ||
        request.segments.back().size() > kMaxSegment) {
      return std::nullopt;
    }
  }
  if (!reader.U8(&end) || end > 2 || !reader.Empty()) return std::nullopt;
  request.end = static_cast<End>(end);
  return request;
}

struct Connection {
  std::string sent;
  bool alive = true;
};

class ScriptedSocket : public net::TransportClientSocket {
 public:
  ScriptedSocket(const Request& request, Connection* connection) : request_(&request), connection_(connection) {}
  ~ScriptedSocket() override { connection_->alive = false; }

  int Bind(const net::IPEndPoint&) override { return net::OK; }
  int Connect(net::CompletionOnceCallback) override {
    connected_ = true;
    return net::OK;
  }
  void Disconnect() override { connected_ = false; }
  bool IsConnected() const override {
    if (!connected_) return false;
    if (socks_state_ != SocksState::kTunnel) return true;
    return HasUnreadData() || request_->end == End::kHang;
  }
  bool IsConnectedAndIdle() const override { return IsConnected() && !HasUnreadData() && socks_reply_.empty(); }
  int GetPeerAddress(net::IPEndPoint* address) const override {
    *address = net::IPEndPoint(net::IPAddress(127, 0, 0, 1), 1080);
    return net::OK;
  }
  int GetLocalAddress(net::IPEndPoint* address) const override {
    *address = net::IPEndPoint(net::IPAddress(127, 0, 0, 1), 40000);
    return net::OK;
  }
  const net::NetLogWithSource& NetLog() const override { return net_log_; }
  bool WasEverUsed() const override { return used_; }
  net::NextProto GetNegotiatedProtocol() const override { return net::NextProto::kProtoUnknown; }
  bool GetSSLInfo(net::SSLInfo*) override { return false; }
  int64_t GetTotalReceivedBytes() const override { return received_; }
  void ApplySocketTag(const net::SocketTag&) override {}
  int SetReceiveBufferSize(int32_t) override { return net::OK; }
  int SetSendBufferSize(int32_t) override { return net::OK; }

  int Read(net::IOBuffer* buf, int buf_len, net::CompletionOnceCallback callback) override {
    used_ = true;
    if (!socks_reply_.empty()) {
      size_t size = std::min<size_t>(buf_len, socks_reply_.size());
      buf->span().copy_prefix_from(base::as_byte_span(socks_reply_).first(size));
      socks_reply_.erase(0, size);
      return static_cast<int>(size);
    }
    if (socks_state_ != SocksState::kTunnel) return net::ERR_UNEXPECTED;
    if (segment_ < request_->segments.size()) {
      const std::string& segment = request_->segments[segment_];
      size_t size = std::min<size_t>(buf_len, segment.size() - offset_);
      buf->span().copy_prefix_from(base::as_byte_span(segment).subspan(offset_, size));
      offset_ += size;
      received_ += size;
      if (offset_ == segment.size()) {
        ++segment_;
        offset_ = 0;
      }
      return static_cast<int>(size);
    }
    switch (request_->end) {
      case End::kClose:
        return 0;
      case End::kReset:
        return net::ERR_CONNECTION_RESET;
      case End::kHang:
        pending_read_ = std::move(callback);
        return net::ERR_IO_PENDING;
    }
  }

  int Write(net::IOBuffer* buf, int buf_len, net::CompletionOnceCallback, const net::NetworkTrafficAnnotationTag&) override {
    used_ = true;
    std::string_view data = base::as_string_view(buf->first(static_cast<size_t>(buf_len)));
    if (socks_state_ == SocksState::kGreeting) {
      socks_state_ = SocksState::kConnect;
      socks_reply_.append("\x05\x00", 2);
    } else if (socks_state_ == SocksState::kConnect) {
      socks_state_ = SocksState::kTunnel;
      socks_reply_.append("\x05\x00\x00\x01\x00\x00\x00\x00\x00\x00", 10);
    } else {
      connection_->sent.append(data);
    }
    return buf_len;
  }

 private:
  enum class SocksState { kGreeting, kConnect, kTunnel };

  bool HasUnreadData() const { return segment_ < request_->segments.size(); }

  raw_ptr<const Request> request_;
  raw_ptr<Connection> connection_;
  net::NetLogWithSource net_log_;
  SocksState socks_state_ = SocksState::kGreeting;
  std::string socks_reply_;
  size_t segment_ = 0;
  size_t offset_ = 0;
  int64_t received_ = 0;
  bool connected_ = false;
  bool used_ = false;
  net::CompletionOnceCallback pending_read_;
};

class ScriptedSocketFactory : public net::ClientSocketFactory {
 public:
  explicit ScriptedSocketFactory(const Request& request) : request_(&request) {}

  std::unique_ptr<net::DatagramClientSocket> CreateDatagramClientSocket(net::DatagramSocket::BindType,
                                                                        net::handles::NetworkHandle, net::NetLog*,
                                                                        const net::NetLogSource&) override {
    std::cerr << "unexpected datagram socket\n";
    std::abort();
  }
  std::unique_ptr<net::TransportClientSocket> CreateTransportClientSocket(
      const net::AddressList&, net::handles::NetworkHandle, std::unique_ptr<net::SocketPerformanceWatcher>,
      net::NetworkQualityEstimator*, net::NetLog*, const net::NetLogSource&) override {
    connections_.push_back(std::make_unique<Connection>());
    return std::make_unique<ScriptedSocket>(*request_, connections_.back().get());
  }
  std::unique_ptr<net::SSLClientSocket> CreateSSLClientSocket(net::SSLClientContext*, std::unique_ptr<net::StreamSocket>,
                                                              const net::HostPortPair&, const net::SSLConfig&) override {
    std::cerr << "unexpected TLS socket\n";
    std::abort();
  }

  const std::vector<std::unique_ptr<Connection>>& connections() const { return connections_; }

 private:
  raw_ptr<const Request> request_;
  std::vector<std::unique_ptr<Connection>> connections_;
};

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

struct Completion {
  bool done = false;
  int result = net::ERR_IO_PENDING;
};

int Await(int rv, Completion& completion) {
  if (rv != net::ERR_IO_PENDING) return rv;
  base::RunLoop().RunUntilIdle();
  return completion.done ? completion.result : net::ERR_IO_PENDING;
}

net::CompletionOnceCallback Record(Completion& completion) {
  completion = Completion();
  return base::BindOnce(
      [](Completion* completion, int result) {
        completion->done = true;
        completion->result = result;
      },
      &completion);
}

std::string Execute(std::string_view input) {
  std::optional<Request> decoded = Decode(input);
  if (!decoded) return "invalid record\n";
  const Request& request = *decoded;
  GURL url(request.url);
  if (!url.is_valid() || url.spec() != request.url || !url.SchemeIs("http")) return "invalid url\n";
  if (!net::HttpUtil::IsToken(request.method)) return "invalid method\n";
  for (const auto& [name, value] : request.headers) {
    if (!net::HttpUtil::IsValidHeaderName(name) || !net::HttpUtil::IsValidHeaderValue(value)) return "invalid header\n";
  }
  GURL referrer(request.referrer);
  if (!request.referrer.empty() && (!referrer.is_valid() || referrer.spec() != request.referrer)) return "invalid referrer\n";
  if (!net::HttpUtil::IsValidHeaderValue(request.user_agent) || !net::HttpUtil::IsValidHeaderValue(request.accept_language) ||
      !net::HttpUtil::IsValidHeaderValue(request.cookie_line)) {
    return "invalid header\n";
  }

  ScriptedSocketFactory factory(request);
  net::URLRequestContextBuilder builder;
  builder.DisableHttpCache();
  builder.set_enable_brotli(true);
  builder.set_enable_zstd(true);
  net::ProxyConfig proxy_config;
  proxy_config.proxy_rules().ParseFromString("socks5://127.0.0.1:1080");
  proxy_config.proxy_rules().bypass_rules.ParseFromString("<-loopback>");
  builder.set_proxy_resolution_service(net::ConfiguredProxyResolutionService::CreateFixedForTest(
      net::ProxyConfigWithAnnotation(proxy_config, TRAFFIC_ANNOTATION_FOR_TESTS)));
  builder.set_client_socket_factory_for_testing(&factory);
  builder.SetCertVerifier(std::make_unique<RejectingCertVerifier>());
  std::unique_ptr<net::URLRequestContext> context = builder.Build();

  net::HttpRequestInfo info;
  info.url = url;
  info.method = request.method;
  info.load_flags = request.load_flags;
  info.traffic_annotation = net::MutableNetworkTrafficAnnotationTag(TRAFFIC_ANNOTATION_FOR_TESTS);
  for (const auto& [name, value] : request.headers) info.extra_headers.SetHeader(name, value);
  info.extra_headers.RemoveHeader(net::HttpRequestHeaders::kReferer);
  if (referrer.is_valid()) info.extra_headers.SetHeader(net::HttpRequestHeaders::kReferer, referrer.spec());
  info.extra_headers.SetHeaderIfMissing(net::HttpRequestHeaders::kUserAgent, request.user_agent);
  info.extra_headers.SetAcceptEncodingIfMissing(url, std::nullopt, true, true);
  if (!request.accept_language.empty()) {
    info.extra_headers.SetHeaderIfMissing(net::HttpRequestHeaders::kAcceptLanguage, request.accept_language);
  }
  if (!request.cookie_line.empty()) info.extra_headers.SetHeader(net::HttpRequestHeaders::kCookie, request.cookie_line);

  std::unique_ptr<net::UploadDataStream> upload;
  if (request.body_kind == 1) {
    upload = net::ElementsUploadDataStream::CreateWithReader(
        std::make_unique<net::UploadBytesElementReader>(base::as_byte_span(request.body[0])));
  } else if (request.body_kind == 2) {
    auto chunked = std::make_unique<net::ChunkedUploadDataStream>(0);
    for (size_t i = 0; i < request.body.size(); ++i) {
      chunked->AppendData(base::as_byte_span(request.body[i]), i + 1 == request.body.size());
    }
    if (request.body.empty()) chunked->AppendData({}, true);
    upload = std::move(chunked);
  }
  info.upload_data_stream = upload.get();

  std::string body;
  std::string start_result;
  std::string end_result = "-";
  std::string response;
  {
    std::unique_ptr<net::HttpTransaction> transaction =
        context->http_transaction_factory()->CreateTransaction(net::DEFAULT_PRIORITY);
    Completion completion;
    int rv = Await(transaction->Start(&info, Record(completion), net::NetLogWithSource()), completion);
    start_result = net::ErrorToShortString(rv);
    if (rv == net::OK) {
      const net::HttpResponseHeaders* headers = transaction->GetResponseInfo()->headers.get();
      if (!headers) {
        std::cerr << "no response headers\n";
        std::abort();
      }
      net::HttpVersion version = headers->GetHttpVersion();
      response += "version " + std::to_string(version.major_value()) + "." + std::to_string(version.minor_value()) + "\n";
      response += "code " + std::to_string(headers->response_code()) + "\n";
      response += "line " + headers->GetStatusLine() + "\n";
      size_t iter = 0;
      std::string name, value;
      while (headers->EnumerateHeaderLines(&iter, &name, &value)) response += "header " + name + ": " + value + "\n";
      auto buffer = base::MakeRefCounted<net::IOBufferWithSize>(kBodyReadSize);
      for (;;) {
        rv = Await(transaction->Read(buffer.get(), kBodyReadSize, Record(completion)), completion);
        if (rv > 0) {
          body.append(base::as_string_view(buffer->first(static_cast<size_t>(rv))));
          continue;
        }
        end_result = net::ErrorToShortString(rv);
        break;
      }
    }
  }
  base::RunLoop().RunUntilIdle();
  bool alive = !factory.connections().empty() && factory.connections().back()->alive;
  context->http_transaction_factory()->GetSession()->CloseIdleConnections("probe");
  base::RunLoop().RunUntilIdle();
  bool reused = alive && !factory.connections().back()->alive;

  std::string output;
  for (const auto& connection : factory.connections()) {
    output += "sent " + std::to_string(connection->sent.size()) + "\n" + connection->sent + "\n";
  }
  output += "start " + start_result + "\n" + response;
  output += "body " + std::to_string(body.size()) + "\n" + body + "\n";
  output += "end " + end_result + "\n";
  output += std::string("reuse ") + (reused ? "yes" : "no") + "\n";
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
                                          base::test::TaskEnvironment::TimeSource::MOCK_TIME,
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
