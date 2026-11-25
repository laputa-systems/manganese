import asyncio
import base64
import itertools
import json
import struct
import sys
import zlib

label, destination = sys.argv[1:]
ids = itertools.count(1)
log = open(destination, 'a', buffering=1)


def record(connection, direction, opcode, payload):
    entry = {'client': label, 'connection': connection, 'direction': direction, 'opcode': opcode}
    try:
        entry['message'] = json.loads(payload)
    except (ValueError, UnicodeDecodeError):
        entry['payload'] = base64.b64encode(payload).decode()
    log.write(json.dumps(entry, separators=(',', ':')) + '\n')


async def frames(reader, writer, connection, direction, compression, no_context, window):
    fragments = bytearray()
    text = False
    compressed = False
    inflater = None
    try:
        while True:
            head = await reader.readexactly(2)
            first, second = head
            length = second & 127
            extra = b''
            if length == 126:
                extra = await reader.readexactly(2)
                length = struct.unpack('!H', extra)[0]
            elif length == 127:
                extra = await reader.readexactly(8)
                length = struct.unpack('!Q', extra)[0]
            mask = await reader.readexactly(4) if second & 128 else b''
            payload = await reader.readexactly(length)
            writer.write(head + extra + mask + payload)
            await writer.drain()
            decoded = bytes(v ^ mask[i % 4] for i, v in enumerate(payload)) if mask else payload
            opcode = first & 15
            record(connection, direction, opcode, decoded)
            if opcode in (1, 2):
                fragments = bytearray(decoded)
                text = opcode == 1
                compressed = bool(first & 64)
            elif opcode == 0:
                fragments.extend(decoded)
            if first & 128 and text and opcode in (0, 1):
                if compressed:
                    if not compression:
                        raise RuntimeError('compressed frame without negotiated compression')
                    if inflater is None:
                        inflater = zlib.decompressobj(-window)
                    message = inflater.decompress(bytes(fragments) + b'\x00\x00\xff\xff')
                    if no_context:
                        inflater = None
                else:
                    message = fragments
                record(connection, direction, 'message', message)
                text = False
            if opcode == 8:
                break
    except (asyncio.IncompleteReadError, ConnectionError):
        pass
    finally:
        writer.close()


async def handle(client, client_writer):
    connection = next(ids)
    upstream_writer = None
    try:
        header = await client.readuntil(b'\r\n\r\n')
        upstream, upstream_writer = await asyncio.open_connection('127.0.0.1', 9221)
        upstream_writer.write(header)
        await upstream_writer.drain()
        response = await upstream.readuntil(b'\r\n\r\n')
        client_writer.write(response)
        await client_writer.drain()
        if response.startswith(b'HTTP/1.1 101'):
            extension = next((line.lower() for line in response.split(b'\r\n')
                              if line.lower().startswith(b'sec-websocket-extensions:')), b'')
            compression = b'permessage-deflate' in extension
            def options(side):
                marker = side + b'_max_window_bits='
                value = next((part.strip()[len(marker):] for part in extension.split(b';')
                              if part.strip().startswith(marker)), b'15')
                return side + b'_no_context_takeover' in extension, int(value)
            client_options = options(b'client')
            server_options = options(b'server')
            await asyncio.gather(frames(client, upstream_writer, connection, 'sent', compression, *client_options),
                                 frames(upstream, client_writer, connection, 'received', compression, *server_options))
        else:
            headers = response.lower().split(b'\r\n')
            length = next((int(h.split(b':', 1)[1]) for h in headers if h.startswith(b'content-length:')), 0)
            body = await upstream.readexactly(length)
            if b'/json/' in header.split(b'\r\n', 1)[0] and b'webSocketDebuggerUrl' in body:
                data = json.loads(body)
                entries = data if isinstance(data, list) else [data]
                for entry in entries:
                    url = entry.get('webSocketDebuggerUrl')
                    if url and not url.startswith('ws://127.0.0.1:9229/'):
                        raise RuntimeError(f'wrong proxy URL: {url}')
                    if url:
                        log.write(json.dumps({'client':label, 'connection':connection, 'direction':'received', 'opcode':'http', 'url':url}) + '\n')
            client_writer.write(body)
            await client_writer.drain()
    except (asyncio.IncompleteReadError, ConnectionError) as error:
        print(f'connection {connection}: {error}', file=sys.stderr, flush=True)
    finally:
        if upstream_writer:
            upstream_writer.close()
        client_writer.close()


async def main():
    server = await asyncio.start_server(handle, '127.0.0.1', 9229)
    async with server:
        await server.serve_forever()


if __name__ == '__main__':
    asyncio.run(main())
