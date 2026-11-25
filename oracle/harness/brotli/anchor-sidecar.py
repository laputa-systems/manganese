import base64
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
sys.path.insert(0, '/launch')
from cdp import Cdp

bodies = [base64.b64decode(value) for value in json.loads(sys.argv[1])]
PAGE = b'<!doctype html><title>brotli</title>'


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != '/' and not self.path[1:].isdigit():
            self.send_error(404)
            return
        encoded = self.path != '/'
        body = bodies[int(self.path[1:])] if encoded else PAGE
        self.send_response(200)
        self.send_header('Content-Type', 'application/octet-stream' if encoded else 'text/html')
        if encoded:
            self.send_header('Content-Encoding', 'br')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


server = ThreadingHTTPServer(('127.0.0.1', 18765), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
client = Cdp.connect_when_ready()
try:
    page = client.page()
    client.send('Page.enable', session=page)
    client.send('Page.navigate', {'url': 'http://127.0.0.1:18765/'}, session=page)
    client.wait_event('Page.loadEventFired', session=page)
    source = '''(async () => { const result = []; for (let i = 0; i < COUNT; i++) { try { const bytes = new Uint8Array(await (await fetch('/' + i)).arrayBuffer()); let text = ''; for (let j = 0; j < bytes.length; j += 8192) text += String.fromCharCode(...bytes.subarray(j, j + 8192)); result.push(btoa(text)); } catch (error) { result.push(null); } } return result; })()'''.replace('COUNT', str(len(bodies)))
    print(json.dumps(client.evaluate(page, source, timeout=110)))
finally:
    client.close()
    server.shutdown()
