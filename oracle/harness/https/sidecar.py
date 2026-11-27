import base64
import json
import socketserver
import ssl
import struct
import sys
import threading
import time

sys.path.insert(0, '/launch')
from cdp import Cdp

CONFIG = json.loads(sys.argv[1])
CASES = {case['host']: case for case in CONFIG['cases']}
PAGE = base64.b64decode(CONFIG['page'])
SOCKS = CONFIG['socks']
HANDSHAKES = {case['name']: [] for case in CONFIG['cases']}
CONNECTS = {case['name']: [] for case in CONFIG['cases']}
CLIENTHELLOS = {}
H2FRAMES = {}
H2SERVER = {}
LOCK = threading.Lock()


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def context(case):
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.set_ciphers('DEFAULT:@SECLEVEL=0')
    if case.get('http2'):
        tls.set_alpn_protocols(['h2'])
    tls.load_cert_chain(f"/fixture/{case['chain']}", f"/fixture/{case['key']}")
    return tls


class Channel:
    def __init__(self, case, request, deadline):
        self.incoming = ssl.MemoryBIO()
        self.outgoing = ssl.MemoryBIO()
        self.tls = context(case).wrap_bio(self.incoming, self.outgoing, server_side=True)
        self.request = request
        self.deadline = deadline
        self.clienthello = None

    def pump(self):
        data = self.outgoing.read()
        if data:
            self.request.sendall(data)

    def feed(self):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError('tls deadline')
        self.request.settimeout(remaining)
        chunk = self.request.recv(65536)
        if not chunk:
            raise ConnectionError('closed')
        self.incoming.write(chunk)
        return chunk

    def handshake(self):
        first = None
        try:
            self.tls.do_handshake()
        except ssl.SSLWantReadError:
            header = None
            while header is None:
                header = self.exact_record_header()
            length = int.from_bytes(header[3:5], 'big')
            first = header + self.exact_bytes(length)
            self.clienthello = first
            self.incoming.write(first)
            while True:
                try:
                    self.tls.do_handshake()
                    break
                except ssl.SSLWantReadError:
                    self.pump()
                    self.feed()

    def exact_record_header(self):
        data = b''
        while len(data) < 5:
            chunk = self.request.recv(5 - len(data))
            if not chunk:
                raise ConnectionError('closed')
            data += chunk
        return data

    def exact_bytes(self, count):
        data = b''
        while len(data) < count:
            chunk = self.request.recv(count - len(data))
            if not chunk:
                raise ConnectionError('closed')
            data += chunk
        return data

    def recv(self):
        while True:
            try:
                return self.tls.read(65536)
            except ssl.SSLWantReadError:
                self.pump()
                self.feed()

    def send(self, data):
        while data:
            try:
                written = self.tls.write(data)
            except ssl.SSLWantWriteError:
                self.pump()
                continue
            data = data[written:]
            self.pump()

    def close(self):
        try:
            self.tls.unwrap()
        except (ssl.SSLError, OSError, ValueError):
            pass
        self.request.close()


def h2_frame(kind, flags, stream, payload):
    return len(payload).to_bytes(3, 'big') + bytes([kind, flags]) + struct.pack('>I', stream) + payload


def h2_exchange(channel, name):
    data = bytearray()

    def fill(length):
        while len(data) < length:
            try:
                chunk = channel.recv()
            except TimeoutError:
                with LOCK:
                    H2FRAMES[name] = bytes(data).hex()
                raise
            if not chunk:
                with LOCK:
                    H2FRAMES[name] = bytes(data).hex()
                return False
            data.extend(chunk)
        return True

    if not fill(24):
        return
    channel.send(h2_frame(4, 0, 0, b''))
    offset = 24
    while True:
        if not fill(offset + 9):
            return
        length = int.from_bytes(data[offset:offset + 3], 'big')
        if not fill(offset + 9 + length):
            return
        kind, flags = data[offset + 3], data[offset + 4]
        stream = int.from_bytes(data[offset + 5:offset + 9], 'big')
        if kind == 4 and not flags & 1:
            channel.send(h2_frame(4, 1, 0, b''))
        if kind == 1:
            block = b'\x88\x00\x0econtent-length\x010\x00\x0ccontent-type\x09text/html'
            channel.send(h2_frame(1, 4, stream, block))
            channel.send(h2_frame(0, 1, stream, b''))
            with LOCK:
                H2SERVER[name] = {'stream': stream, 'frames': [h2_frame(1, 4, stream, block).hex(),
                                                               h2_frame(0, 1, stream, b'').hex()]}
            offset += 9 + length
            channel.deadline = time.monotonic() + 2
            for _ in range(4):
                try:
                    if not fill(offset + 9):
                        break
                    trailing_length = int.from_bytes(data[offset:offset + 3], 'big')
                    if not fill(offset + 9 + trailing_length):
                        break
                    if data[offset + 3] == 4 and data[offset + 4] & 1:
                        break
                    offset += 9 + trailing_length
                except TimeoutError:
                    break
            with LOCK:
                H2FRAMES[name] = bytes(data).hex()
            return
        offset += 9 + length


class Socks(socketserver.BaseRequestHandler):
    def exact(self, count):
        data = b''
        while len(data) < count:
            chunk = self.request.recv(count - len(data))
            if not chunk:
                raise ConnectionError('closed')
            data += chunk
        return data

    def handle(self):
        self.request.settimeout(20)
        deadline = time.monotonic() + 20
        try:
            version, count = self.exact(2)
            self.exact(count)
            if version != 5:
                return
            self.request.sendall(b'\x05\x00')
            _, command, _, kind = self.exact(4)
            if command != 1:
                return
            if kind == 3:
                host = self.exact(self.exact(1)[0]).decode('ascii')
            elif kind == 1:
                host = '.'.join(str(byte) for byte in self.exact(4))
            else:
                return
            port = struct.unpack('!H', self.exact(2))[0]
            case = CASES.get(host)
            if case is None or port != SOCKS['https_port']:
                reply = b'\x05\x02\x00\x01\x00\x00\x00\x00\x00\x00'
                self.request.sendall(reply)
                return
            reply = b'\x05\x00\x00\x01\x7f\x00\x00\x01' + struct.pack('!H', SOCKS['https_port'])
            self.request.sendall(reply)
            with LOCK:
                CONNECTS[case['name']].append({'host': host, 'port': port, 'reply': reply.hex()})
            if CONFIG.get('capture_clienthello'):
                channel = Channel(case, self.request, deadline)
                channel.handshake()
                with LOCK:
                    HANDSHAKES[case['name']].append('handshake completed')
                    CLIENTHELLOS[case['name']] = channel.clienthello.hex()
                request = b''
                while b'\r\n\r\n' not in request:
                    chunk = channel.recv()
                    if not chunk:
                        return
                    request += chunk
                channel.send(PAGE)
                channel.close()
                return
            if case.get('http2'):
                channel = Channel(case, self.request, deadline)
                channel.handshake()
                with LOCK:
                    HANDSHAKES[case['name']].append('handshake completed')
                try:
                    h2_exchange(channel, case['name'])
                except Exception as error:
                    with LOCK:
                        if case['name'] not in H2FRAMES:
                            H2FRAMES[case['name']] = f'error: {type(error).__name__}: {error}'
                channel.close()
                return
            try:
                tls = context(case).wrap_socket(self.request, server_side=True)
            except (ssl.SSLError, OSError) as error:
                with LOCK:
                    HANDSHAKES[case['name']].append(f'handshake ended: {type(error).__name__}')
                return
            with LOCK:
                HANDSHAKES[case['name']].append('handshake completed')
            request = b''
            while b'\r\n\r\n' not in request:
                chunk = tls.recv(65536)
                if not chunk:
                    return
                request += chunk
            tls.sendall(PAGE)
            tls.close()
        except (OSError, ConnectionError, ValueError):
            return


def observe(client, page, url):
    requests, results = set(), {}
    deadline = time.monotonic() + 30
    client.send('Page.navigate', {'url': url}, session=page)
    while time.monotonic() < deadline:
        try:
            event = client.wait_event_any(page, deadline)
        except TimeoutError:
            break
        method, params = event.get('method'), event.get('params', {})
        request_id = params.get('requestId')
        if method == 'Network.requestWillBeSent' and params['request']['url'] == url:
            requests.add(request_id)
        elif request_id in requests and method == 'Network.responseReceived':
            details = params['response'].get('securityDetails', {})
            results['ct'] = details.get('certificateTransparencyCompliance')
        elif request_id in requests and method == 'Network.loadingFinished':
            results['error'] = 'OK'
            chain = client.send('Network.getCertificate', {'origin': url.rstrip('/')}, session=page)
            results['chain'] = chain['tableNames']
            return results
        elif request_id in requests and method == 'Network.loadingFailed':
            results['error'] = params['errorText']
            return results
    results['timeout'] = True
    return results


def wait_event_any(self, session, deadline):
    while True:
        for event in list(self.events):
            if event.get('sessionId') == session and 'method' in event:
                self.events.remove(event)
                return event
        if time.monotonic() >= deadline:
            raise TimeoutError('no event')
        self.events.append(self._receive(deadline))


Cdp.wait_event_any = wait_event_any
server = Server((SOCKS['host'], SOCKS['port']), Socks)
threading.Thread(target=server.serve_forever, daemon=True).start()
client = Cdp.connect_when_ready()
observed = {}
try:
    for case in CONFIG['cases']:
        page = client.page()
        client.send('Network.enable', session=page)
        client.send('Network.setCacheDisabled', {'cacheDisabled': True}, session=page)
        observed[case['name']] = {'started': int(time.time()), **observe(client, page, f"https://{case['host']}{case['path']}")}
        client.send('Target.closeTarget', {'targetId': client.send('Target.getTargetInfo', session=page)['targetInfo']['targetId']})
finally:
    server.shutdown()
    server.server_close()
    client.close()
with LOCK:
    print(json.dumps({'observed': observed, 'handshakes': HANDSHAKES, 'connects': CONNECTS,
                      'clienthellos': CLIENTHELLOS, 'http2': H2FRAMES, 'http2_server': H2SERVER,
                      'socks_bind': f"{SOCKS['host']}:{SOCKS['port']}"}))
