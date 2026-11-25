import asyncio
import importlib.util
import json
import struct
import sys
import tempfile
import unittest
import zlib
from pathlib import Path


class Writer:
    def __init__(self):
        self.data = b''

    def write(self, data):
        self.data += data

    async def drain(self):
        pass

    def close(self):
        pass


class ProxyTest(unittest.TestCase):
    def test_compressed_messages_with_context_takeover(self):
        directory = Path(__file__).resolve().parents[3] / '.work/tmp'
        directory.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=directory) as scratch:
            path = Path(scratch) / 'frames.jsonl'
            old_argv = sys.argv
            try:
                sys.argv = ['proxy.py', 'client', str(path)]
                spec = importlib.util.spec_from_file_location('cdp_proxy_test', Path(__file__).with_name('proxy.py'))
                proxy = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(proxy)
            finally:
                sys.argv = old_argv
            writer = Writer()
            compressor = zlib.compressobj(wbits=-15)
            wire = b''
            for identifier in (1, 2):
                message = json.dumps({'id':identifier, 'method':'Target.getTargets'}).encode()
                payload = (compressor.compress(message) + compressor.flush(zlib.Z_SYNC_FLUSH))[:-4]
                wire += struct.pack('BB', 0xc1, len(payload)) + payload

            async def run():
                reader = asyncio.StreamReader()
                reader.feed_data(wire)
                reader.feed_eof()
                await proxy.frames(reader, writer, 1, 'sent', True, False, 15)

            try:
                asyncio.run(run())
            finally:
                proxy.log.close()
            records = [json.loads(line) for line in path.read_text().splitlines()]
            self.assertEqual(writer.data, wire)
            self.assertEqual([item['message']['id'] for item in records if item['opcode'] == 'message'], [1, 2])


if __name__ == '__main__':
    unittest.main()
