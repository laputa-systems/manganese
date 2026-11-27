import importlib.util
import json
import pathlib
import struct
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT / 'oracle/harness/https'))
import fixture
import wire

ORACLE = ROOT / '.work/tmp/http2/oracle'
CANDIDATE = ROOT / '.work/tmp/target/x86_64-unknown-linux-musl/debug/http2'
CERT_ORACLE = ROOT / '.work/tmp/cert-verify/oracle'
HOST = 'verified.https.test'
PREFACE = b'PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n'
STATIC_NAMES = [
    b':authority', b':method', b':method', b':path', b':path', b':scheme', b':scheme',
    b':status', b':status', b':status', b':status', b':status', b':status', b':status',
]

_spec = importlib.util.spec_from_file_location('http2_wire', str(HERE / 'wire.py'))
wire = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(wire)


def short(value):
    value = value.encode() if isinstance(value, str) else value
    return struct.pack('>H', len(value)) + value


def request_record():
    item = {'method': 'GET', 'scheme': 'https', 'host': HOST, 'path': '/', 'headers': [],
            'priority': 5, 'status': 200}
    return wire.encode({'requests': [item], 'settings': [], 'window_update': 65535, 'end': 'close'})


def read_int(data, offset, prefix_bits):
    limit = (1 << prefix_bits) - 1
    value = data[offset] & limit
    offset += 1
    if value < limit:
        return value, offset
    shift = 0
    while True:
        byte = data[offset]
        offset += 1
        value += (byte & 0x7f) << shift
        if not byte & 0x80:
            return value, offset
        shift += 7


def read_string(data, offset):
    length, offset = read_int(data, offset, 7)
    return offset + length


def header_names(block):
    names = []
    offset = 0
    while offset < len(block) and len(names) < 4:
        first = block[offset]
        offset += 1
        if first & 0x80:
            index, offset = read_int(block, offset - 1, 7)
            offset += 0
            names.append(('indexed', STATIC_NAMES[index - 1].decode() if index <= len(STATIC_NAMES) else str(index)))
            continue
        if first & 0x40:
            index, offset = read_int(block, offset - 1, 6)
            if index == 0:
                offset = read_string(block, offset)
            offset = read_string(block, offset)
            name = STATIC_NAMES[index - 1].decode() if 0 < index <= len(STATIC_NAMES) else 'dynamic'
            names.append(('indexed-literal', name))
            continue
        if first & 0x20:
            _, offset = read_int(block, offset - 1, 5)
            continue
        index, offset = read_int(block, offset - 1, 4)
        if index == 0:
            offset = read_string(block, offset)
        offset = read_string(block, offset)
        name = STATIC_NAMES[index - 1].decode() if 0 < index <= len(STATIC_NAMES) else 'dynamic'
        names.append(('literal', name))
    return names


def profile(raw):
    if len(raw) < 24 or raw[:24] != PREFACE:
        raise SystemExit('anchor: invalid connection preface')
    settings = None
    window = None
    headers = None
    ack = False
    offset = 24
    while offset + 9 <= len(raw):
        length = int.from_bytes(raw[offset:offset + 3], 'big')
        kind, flags = raw[offset + 3], raw[offset + 4]
        stream = int.from_bytes(raw[offset + 5:offset + 9], 'big')
        payload = raw[offset + 9:offset + 9 + length]
        offset += 9 + length
        if kind == 4 and not flags & 1:
            settings = [(int.from_bytes(payload[index:index + 2], 'big'),
                         int.from_bytes(payload[index + 2:index + 6], 'big'))
                        for index in range(0, len(payload), 6)]
        elif kind == 4:
            ack = True
        elif kind == 8:
            window = int.from_bytes(payload, 'big')
        elif kind == 1 and headers is None:
            headers = {'flags': flags, 'stream': stream,
                       'weight': payload[4] if flags & 0x20 else None,
                       'names': header_names(payload[5:])}
    if settings is None or window is None or headers is None or not ack:
        raise SystemExit('anchor: incomplete HTTP/2 client profile')
    return settings, window, headers


def peer(path, record):
    process = subprocess.Popen([str(path)], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    try:
        process.stdin.write(b'MGB1')
        process.stdin.flush()
        if process.stdout.read(4) != b'MGB1':
            raise SystemExit(f'anchor: {path} wrote an invalid protocol header')
        process.stdin.write(struct.pack('>I', len(record)) + record)
        process.stdin.flush()
        size = struct.unpack('>I', process.stdout.read(4))[0]
        value = process.stdout.read(size)
        process.stdin.close()
        if process.wait() != 0:
            raise SystemExit(f'anchor: {path} failed')
        line = value.decode().split('\n')[0]
        return bytes.fromhex(line.split()[1])
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()


def main():
    for path in (ORACLE, CANDIDATE, CERT_ORACLE):
        if not path.is_file():
            raise SystemExit(f'anchor: {path.relative_to(ROOT)} is missing')
    cases, policy, trust, moment = fixture.sample()
    result = fixture.run(cases, policy, trust, oracle=CERT_ORACLE, label='http2-anchor',
                         pinned_time=moment, http2_cases=['verified'])
    evidence = result['evidence']
    if evidence['failures']:
        for failure in evidence['failures']:
            print(failure, file=sys.stderr)
        raise SystemExit('anchor: the HTTPS fixture did not verify')
    comparison = evidence['comparisons'][0]
    if comparison['browser'].get('error') != 'OK':
        raise SystemExit('anchor: no verified HTTP/2 navigation')
    raw = bytes.fromhex(evidence['http2']['verified'])
    record = request_record()
    browser = profile(raw)
    harness = profile(peer(ORACLE, record))
    candidate = profile(peer(CANDIDATE, record))
    if browser != harness or browser != candidate:
        for label, value in (('browser', browser), ('harness', harness), ('candidate', candidate)):
            print(f'anchor {label} {json.dumps(value, sort_keys=True, default=str)}', file=sys.stderr)
        raise SystemExit('anchor: HTTP/2 client profiles disagree')
    print(f"anchor 1/1: browser, source harness and candidate agree on the modeled HTTP/2 client profile")
    print(f"anchor settings {json.dumps(browser[0])} window {browser[1]} weight {browser[2]['weight']}")
    print(f"anchor pseudo-headers {json.dumps(browser[2]['names'])} ack yes")
    print(f"anchor browser {json.dumps(comparison['browser'], sort_keys=True)}")
    print(f"anchor handshakes {json.dumps(comparison['handshakes'])}")
    print(f"anchor evidence {evidence['run_dir']}/evidence.json")


if __name__ == '__main__':
    main()
