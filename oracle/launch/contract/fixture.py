import http.server
import hashlib
import json
import select
import socket
import socketserver
import struct
import threading
import time
from pathlib import Path

LOG = Path('/evidence/connects.jsonl')
READY = Path('/evidence/readiness')

class HTTP(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/sw.js':
            body = b'self.addEventListener("fetch",()=>{});'
            content_type = 'text/javascript'
        elif self.path in ('/profile', '/profile?seed'):
            body = b'<!doctype html><title>profile fixture</title><a id="download" href="/profile-download">download</a>'
            content_type = 'text/html'
        elif self.path == '/profile-download':
            body = b'headed download evidence'
            content_type = 'application/octet-stream'
        elif self.path == '/workload':
            body = b'''<!doctype html><title>Client workload</title><h1>Client workload</h1>
<form action="/submit" method="post"><label>Text <input name="text" id="text"></label><button id="submit">Submit</button></form>
<script>window.addEventListener('message',e=>{if(e.data==='cross-ready')window.crossReady=true})</script>
<iframe id="same" src="/same"></iframe><iframe id="cross" src="http://frame.test/cross"></iframe>
<input id="upload" type="file"><button id="send-upload" onclick="fetch('/upload',{method:'POST',body:document.querySelector('#upload').files[0]})">Upload</button>
<button id="confirm" onclick="document.body.dataset.confirm=confirm('Confirm workload')">Confirm</button>
<button id="popup" onclick="window.open('/popup')">Popup</button>
<button id="blob" onclick="let a=document.createElement('a');a.href=window.URL.createObjectURL(new Blob(['client download']));a.download='client-download.txt';a.click()">Download</button>'''
            content_type = 'text/html'
        elif self.path == '/same':
            body = b'<!doctype html><p id="same-text">same-origin frame</p>'
            content_type = 'text/html'
        elif self.path == '/cross':
            body = b'<!doctype html><button id="cross-click" onclick="fetch(\'/frame-click\',{method:\'POST\',body:\'clicked\'})">Cross-origin click</button><script>parent.postMessage("cross-ready","*")</script>'
            content_type = 'text/html'
        else:
            body = b'<!doctype html><title>fixture</title><input id="field"><iframe src="http://frame.test/"></iframe>' if self.headers.get('Host', '').startswith('forms.test') else b'<!doctype html><title>frame</title>'
            content_type = 'text/html'
        self.send_response(200)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(body)))
        if self.path == '/profile?seed':
            self.send_header('Set-Cookie', 'contract_profile=persisted; Max-Age=86400; Path=/; HttpOnly; SameSite=Lax')
        if self.path == '/profile-download':
            self.send_header('Content-Disposition', 'attachment; filename="contract-profile.txt"')
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        data = self.rfile.read(int(self.headers['Content-Length']))
        with (LOG.parent / 'requests.jsonl').open('a') as log:
            log.write(json.dumps({'path':self.path, 'length':len(data), 'sha256':hashlib.sha256(data).hexdigest(),
                                  'body':data.decode('utf-8', errors='replace') if len(data) < 1024 else None}) + '\n')
        self.send_response(200)
        self.send_header('Content-Length', '2')
        self.end_headers()
        self.wfile.write(b'ok')

    def log_message(self, *_args):
        pass

class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True

class Socks(socketserver.BaseRequestHandler):
    def exact(self, size):
        data = b''
        while len(data) < size:
            chunk = self.request.recv(size - len(data))
            if not chunk:
                raise ConnectionError('closed')
            data += chunk
        return data

    def handle(self):
        try:
            version, count = self.exact(2)
            methods = self.exact(count)
            if version != 5 or 0 not in methods:
                return
            self.request.sendall(b'\x05\x00')
            version, command, _, kind = self.exact(4)
            if kind == 3:
                host = self.exact(self.exact(1)[0]).decode('ascii')
            elif kind == 1:
                host = socket.inet_ntop(socket.AF_INET, self.exact(4))
            elif kind == 4:
                host = socket.inet_ntop(socket.AF_INET6, self.exact(16))
            else:
                return
            port = struct.unpack('!H', self.exact(2))[0]
            now = time.monotonic()
            entry = {'version': version, 'command': command, 'address_type': kind, 'host': host, 'port': port,
                     'seconds_after_readiness': now - float(READY.read_text()) if READY.exists() else None}
            with LOG.open('a') as log:
                log.write(json.dumps(entry) + '\n')
            if version != 5 or command != 1 or kind != 3 or host not in ('forms.test', 'frame.test', 'localhost') or port != 80:
                self.request.sendall(b'\x05\x02\x00\x01\x00\x00\x00\x00\x00\x00')
                return
            with socket.create_connection(('127.0.0.1', 18080), timeout=5) as upstream:
                self.request.sendall(b'\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00')
                self.request.settimeout(None)
                upstream.settimeout(None)
                while True:
                    readable, _, _ = select.select([self.request, upstream], [], [], 60)
                    if not readable:
                        break
                    for source in readable:
                        chunk = source.recv(65536)
                        if not chunk:
                            return
                        (upstream if source is self.request else self.request).sendall(chunk)
        except (OSError, ConnectionError, ValueError):
            return

http = Server(('127.0.0.1', 18080), HTTP)
threading.Thread(target=http.serve_forever, daemon=True).start()
Server(('127.0.0.1', 1080), Socks).serve_forever()
