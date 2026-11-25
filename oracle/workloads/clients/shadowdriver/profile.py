import os
import asyncio
import json
import sys

from shadowdriver import connect_browser


async def main():
    browser, tab = await connect_browser(os.environ.get('CDP_ENDPOINT', 'http://127.0.0.1:9221'), stealth=False)
    try:
        await tab.navigate('http://forms.test/profile?seed' if sys.argv[1] == 'seed' else 'http://forms.test/profile', wait='load')
        await tab.document_ready()
        print(json.dumps({'cookies':await tab.get_cookies()}))
    finally:
        await tab.close()
        await browser.close(terminate=False)


asyncio.run(main())
