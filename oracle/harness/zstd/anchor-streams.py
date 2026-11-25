import base64
import json
import struct
import subprocess


def encode(data: bytes, level: int, options: tuple[str, ...] | list[str] = ()) -> bytes:
    return subprocess.run(['zstd', '-q', '-c', f'-{level}', *options], input=data, stdout=subprocess.PIPE, check=True).stdout


text = b'anchor document line\n' * 400 + bytes(range(256)) * 20
documents = [b'', b'a', text, text[:5000], bytes(range(256)) * 300, b'window-boundary-' * 20000]
streams = []
for level, window in ((1, 16), (3, 20), (9, 22), (19, 23)):
    for data in documents:
        for size in (False, True):
            options = [f'--zstd=wlog={window}']
            if size:
                options.append(f'--stream-size={len(data)}')
            streams.append((f'level-{level}-window-{window}-size-{size}-document-{len(data)}', encode(data, level, options)))
encoded = encode(text, 3)
large = encode(b'large-window-' * 800000, 1, ('--long=24',))
streams += [
    ('multiple-frames', encoded + encode(b'second frame', 1)),
    ('skippable-frame', struct.pack('<II', 0x184D2A50, 3) + b'abc' + encoded),
    ('over-window-limit', large),
    ('truncated-checksum', encoded[:-1]),
    ('truncated-frame', encoded[:len(encoded) // 2]),
    ('corrupt-checksum', encoded[:-1] + bytes([encoded[-1] ^ 1])),
    ('trailing-garbage', encoded + b'trailing bytes'),
    ('empty-body', b''),
]
print(json.dumps([[name, base64.b64encode(data).decode()] for name, data in streams]))
