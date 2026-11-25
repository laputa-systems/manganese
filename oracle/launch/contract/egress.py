import json
import sys
import time
sys.path.insert(0, '/launch')
from cdp import Cdp

client = Cdp.connect()
page = client.page('http://forms.test/')
time.sleep(3)
print(json.dumps({'url': client.evaluate(page, 'location.href'), 'frames': client.evaluate(page, 'frames.length')}))
client.evaluate(page, "fetch('https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js', {mode:'no-cors'}).catch(() => {})")
time.sleep(2)
client.close()
