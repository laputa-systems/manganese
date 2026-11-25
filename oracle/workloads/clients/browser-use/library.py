import asyncio
import json
import os
import subprocess
from pathlib import Path

from browser_use import Browser


def cdp_sockets():
    rows = Path('/proc/net/tcp').read_text().splitlines()[1:]
    return sorted((parts[1], parts[2]) for row in rows if (parts := row.split())
                  and parts[3] == '01' and (parts[1].endswith(':2405') or parts[2].endswith(':2405')))


async def main():
    before = cdp_sockets()
    connections = []
    mode = os.environ.get('PROFILE_MODE')
    for _ in range(1 if mode else 10):
        browser = Browser(cdp_url='http://127.0.0.1:9221')
        try:
            environment = await browser.start()
            connections.append(environment)
            if mode:
                commands = [['sh', '/workload/profile.sh', mode]]
            elif len(connections) == 1:
                commands = [['sh', '/workload/workload.sh'], ['sh', '/workload/extra.sh']]
            else:
                commands = []
            for command in commands:
                await asyncio.to_thread(subprocess.run, command, env={**os.environ, **environment}, check=True, timeout=60)
        finally:
            await browser.stop()
    immediate = cdp_sockets()
    for _ in range(50):
        after = cdp_sockets()
        if not after:
            break
        await asyncio.sleep(0.1)
    print(json.dumps({'cycles':len(connections), 'connections':connections, 'before':before,
                      'immediate':immediate, 'after':after}))


asyncio.run(main())
