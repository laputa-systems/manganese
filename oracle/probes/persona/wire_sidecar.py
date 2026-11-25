import sys

sys.path.insert(0, '/launch')
from cdp import Cdp

client = Cdp.connect()
try:
    page = client.page()
    client.send('Page.enable', session=page)
    client.send('Page.navigate', {'url':'http://nav.persona.test/'}, session=page)
    client.wait_event('Page.loadEventFired', session=page, timeout=30)
    with open('/persona/wire.js', encoding='utf-8') as stream:
        client.evaluate(page, stream.read(), timeout=60)
    print('wire requests complete')
finally:
    client.close()
