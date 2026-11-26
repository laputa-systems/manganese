import base64
import json
import socket
import socketserver
import struct
import sys
import threading
import time

sys.path.insert(0, '/launch')
from cdp import Cdp

CASES = {case['host'] + ':' + str(case['port']): case for case in json.loads(sys.argv[1])}
CAPTURED = {name: [] for name in (case['name'] for case in CASES.values())}
LOCK = threading.Lock()
STOP = threading.Event()


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


class Socks(socketserver.BaseRequestHandler):
    def exact(self, count):
        data = b''
        while len(data) < count:
            chunk = self.request.recv(count - len(data))
            if not chunk:
                raise ConnectionError('closed')
            data += chunk
        return data

    def read_request(self):
        data = b''
        while b'\r\n\r\n' not in data:
            chunk = self.request.recv(65536)
            if not chunk:
                return None
            data += chunk
        head, body = data.split(b'\r\n\r\n', 1)
        length = 0
        for line in head.split(b'\r\n')[1:]:
            name, _, value = line.partition(b':')
            if name.strip().lower() == b'content-length':
                length = int(value.strip())
        while len(body) < length:
            body += self.exact(length - len(body))
        return head + b'\r\n\r\n' + body

    def handle(self):
        self.request.settimeout(20)
        try:
            version, count = self.exact(2)
            self.exact(count)
            if version != 5:
                return
            self.request.sendall(b'\x05\x00')
            _, command, _, kind = self.exact(4)
            if command != 1 or kind != 3:
                return
            host = self.exact(self.exact(1)[0]).decode('ascii')
            port = struct.unpack('!H', self.exact(2))[0]
            case = CASES.get(host + ':' + str(port))
            if case is None:
                self.request.sendall(b'\x05\x02\x00\x01\x00\x00\x00\x00\x00\x00')
                return
            self.request.sendall(b'\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00')
            request = self.read_request()
            if request is None:
                return
            if case.get('page') and request.startswith(b'GET / '):
                self.request.sendall(base64.b64decode(case['page']))
                return
            with LOCK:
                CAPTURED[case['name']].append(base64.b64encode(request).decode())
            self.request.sendall(base64.b64decode(case['response']))
            if case['end'] == 'hang':
                STOP.wait(60)
            elif case['end'] == 'reset':
                self.request.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack('ii', 1, 0))
        except (OSError, ConnectionError, ValueError):
            return


def observe(client, page, url, start):
    requests, results = {}, {}
    deadline = time.monotonic() + 30
    start()
    while time.monotonic() < deadline:
        try:
            event = client.wait_event_any(page, deadline)
        except TimeoutError:
            break
        method, params = event.get('method'), event.get('params', {})
        request_id = params.get('requestId')
        if method == 'Network.requestWillBeSent' and params['request']['url'] == url:
            requests[request_id] = True
        elif request_id in requests and method == 'Network.responseReceived':
            response = params['response']
            results['response'] = {'status': response['status'], 'statusText': response['statusText'], 'headers': response['headers']}
        elif request_id in requests and method == 'Network.loadingFinished':
            body = client.send('Network.getResponseBody', {'requestId': request_id}, session=page)
            data = base64.b64decode(body['body']) if body.get('base64Encoded') else body['body'].encode('utf-8')
            results['body'] = base64.b64encode(data).decode()
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
server = Server(('127.0.0.1', 1080), Socks)
threading.Thread(target=server.serve_forever, daemon=True).start()
client = Cdp.connect_when_ready()
observed = {}
try:
    for case in CASES.values():
        page = client.page()
        client.send('Network.enable', session=page)
        client.send('Network.setCacheDisabled', {'cacheDisabled': True}, session=page)
        origin = f"http://{case['host']}" + ('' if case['port'] == 80 else f":{case['port']}")
        if case['kind'] == 'navigate':
            url = origin + case['path']
            observed[case['name']] = observe(client, page, url, lambda: client.send('Page.navigate', {'url': url}, session=page))
        else:
            client.send('Page.enable', session=page)
            client.send('Page.navigate', {'url': origin + '/'}, session=page)
            client.wait_event('Page.loadEventFired', session=page, timeout=30)
            url = origin + case['path']
            script = f"fetch({json.dumps(case['path'])}, {{method: 'POST', body: {json.dumps(case['body'])}, headers: {{'Content-Type': 'text/plain'}}}}).catch(() => null); 'sent'"
            observed[case['name']] = observe(client, page, url, lambda: client.evaluate(page, script))
        client.send('Target.closeTarget', {'targetId': client.send('Target.getTargetInfo', session=page)['targetInfo']['targetId']})
finally:
    STOP.set()
    server.shutdown()
    server.server_close()
    client.close()
with LOCK:
    print(json.dumps({'observed': observed, 'captured': CAPTURED}))
