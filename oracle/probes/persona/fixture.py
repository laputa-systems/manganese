import json
import socket
import socketserver
import struct
import threading
from pathlib import Path

LOG = Path('/evidence/wire.jsonl')
LOCK = threading.Lock()


def record(value):
    with LOCK, LOG.open('a') as stream:
        stream.write(json.dumps(value, sort_keys=True) + '\n')


def take(data, pos, size):
    return data[pos:pos + size], pos + size


def vector(data, pos, size):
    length = int.from_bytes(data[pos:pos + size], 'big')
    return take(data, pos + size, length)


def words(data):
    return [int.from_bytes(data[i:i + 2], 'big') for i in range(0, len(data), 2)]


def grease(value):
    return value & 0x0f0f == 0x0a0a and value >> 8 == value & 255


def norm(value):
    return 'GREASE' if grease(value) else value


def hello(data):
    if data[0] != 22 or data[5] != 1:
        raise ValueError('not ClientHello')
    body = data[9:]
    version = int.from_bytes(body[:2], 'big')
    pos = 34
    _, pos = vector(body, pos, 1)
    cipher, pos = vector(body, pos, 2)
    _, pos = vector(body, pos, 1)
    extensions, pos = vector(body, pos, 2)
    result = {'legacyVersion':version, 'cipherSuites':[norm(x) for x in words(cipher)], 'extensions':[], 'values':{}}
    p = 0
    while p < len(extensions):
        kind = int.from_bytes(extensions[p:p + 2], 'big')
        value, p = vector(extensions, p + 2, 2)
        name = str(norm(kind))
        result['extensions'].append(norm(kind))
        if kind in (10, 13, 50):
            result['values'][name] = [norm(x) for x in words(vector(value, 0, 2)[0])]
        elif kind == 51:
            shares = vector(value, 0, 2)[0]; q = 0; groups = []
            while q < len(shares):
                group = int.from_bytes(shares[q:q + 2], 'big'); key, q = vector(shares, q + 2, 2); groups.append({'group':norm(group),'keyLength':len(key)})
            result['values'][name] = groups
        elif kind in (16, 17513, 17613):
            items = vector(value, 0, 2)[0]; q = 0; names = []
            while q < len(items):
                item, q = vector(items, q, 1); names.append(item.decode('ascii'))
            result['values'][name] = names
        elif kind == 27:
            result['values'][name] = words(vector(value, 0, 1)[0])
        elif kind in (43, 45):
            v = vector(value, 0, 1)[0]; result['values'][name] = [norm(x) for x in words(v)] if kind == 43 else list(v)
        elif kind == 65037:
            result['values'][name] = {'length':len(value), 'type':value[0] if value else None}
        elif kind == 35:
            result['values'][name] = {'length':len(value)}
    return result


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

    def handle(self):
        self.request.settimeout(10)
        try:
            version, count = self.exact(2); methods = self.exact(count)
            if version != 5 or 0 not in methods:
                return
            self.request.sendall(b'\x05\x00')
            version, command, _, kind = self.exact(4)
            if kind != 3 or version != 5 or command != 1:
                return
            host = self.exact(self.exact(1)[0]).decode('ascii'); port = struct.unpack('!H', self.exact(2))[0]
            if not host.endswith('.persona.test') or port not in (80, 443):
                self.request.sendall(b'\x05\x02\x00\x01\x00\x00\x00\x00\x00\x00'); return
            self.request.sendall(b'\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00')
            if port == 443:
                head = self.exact(5); body = self.exact(int.from_bytes(head[3:5], 'big')); record({'kind':'tls','hello':hello(head + body)}); return
            data = b''
            while b'\r\n\r\n' not in data:
                data += self.request.recv(4096)
                if not data:
                    return
            lines = data.split(b'\r\n\r\n', 1)[0].decode('latin1').split('\r\n'); request = lines[0]; headers = []
            for line in lines[1:]:
                key, value = line.split(':', 1); value = value.strip()
                if key.lower() == 'sec-websocket-key':
                    import base64
                    if len(base64.b64decode(value, validate=True)) != 16:
                        raise ValueError('invalid websocket key')
                    value = '<base64-16-bytes>'
                headers.append([key, value])
            record({'kind':'http','host':host,'request':request,'headers':headers})
            cookie = b'Set-Cookie: persona=fixture; Path=/\r\n' if request == 'GET / HTTP/1.1' else b''
            response = b'HTTP/1.1 200 OK\r\n' + cookie + b'Content-Type: text/html\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: x-persona\r\nConnection: close\r\nContent-Length: 0\r\n\r\n'
            self.request.sendall(response)
        except (OSError, ConnectionError, ValueError, IndexError):
            return


server = Server(('127.0.0.1', 1080), Socks)
Path('/evidence/ready').write_text('ready')
server.serve_forever()
