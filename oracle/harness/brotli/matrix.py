import pathlib
import subprocess
from compare import ask, peer, request

root = pathlib.Path(__file__).resolve().parents[3]
encoded_data = [(b'hello world\n' * 1000), bytes(range(256)) * 20]
a = peer(str(root / '.work/tmp/brotli/oracle'))
b = peer(str(root / '.work/tmp/target/x86_64-unknown-linux-musl/debug/brotli'))
cases = 0
failures = 0
for data in encoded_data:
    for quality in (0, 6, 11):
        for window in (10, 22, 25):
            encoded = subprocess.run([str(root / '.work/tmp/brotli/encode'), str(quality), str(window)], input=data, stdout=subprocess.PIPE, check=True).stdout
            for width in (1, 17, 32768):
                chunks = [encoded[i:i + width] for i in range(0, len(encoded), width)] or [b'']
                for output_size in (64, 4096):
                    value = request(0, output_size, b'', chunks)
                    expected, actual = ask(a, value), ask(b, value)
                    cases += 1
                    if expected != actual:
                        failures += 1
                        if failures <= 8:
                            print('divergence', len(data), quality, window, width, output_size, 'oracle', expected[:40].hex(), 'candidate', actual[:40].hex(), flush=True)
            value = request(1, len(data), b'', [encoded])
            expected, actual = ask(a, value), ask(b, value)
            cases += 1
            if expected != actual:
                failures += 1
                if failures <= 8:
                    print('one-shot divergence', len(data), quality, window, 'oracle', expected[:40].hex(), 'candidate', actual[:40].hex(), flush=True)
for process in (a, b):
    process.stdin.close()
    if process.wait():
        raise SystemExit('peer failed')
print('matrix', cases, 'cases', failures, 'divergences')
raise SystemExit(bool(failures))
