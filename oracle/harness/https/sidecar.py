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
LOCK = threading.Lock()


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def context(case):
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.set_ciphers('DEFAULT:@SECLEVEL=0')
    tls.load_cert_chain(f"/fixture/{case['chain']}", f"/fixture/{case['key']}")
    return tls


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
                      'socks_bind': f"{SOCKS['host']}:{SOCKS['port']}"}))
