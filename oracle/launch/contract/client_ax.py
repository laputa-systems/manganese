import json
import sys
import time
from pathlib import Path

sys.path.insert(0, '/launch')
from cdp import Cdp

evidence = Path('/evidence')
for _ in range(300):
    marker = evidence / 'extra-snapshot.json'
    if marker.exists() and marker.stat().st_size:
        break
    time.sleep(0.1)
else:
    raise TimeoutError('client snapshot did not complete')

client = Cdp.connect()
try:
    targets = client.send('Target.getTargets')['targetInfos']
    target = next(item['targetId'] for item in targets if item.get('url') == 'http://forms.test/workload' and item['type'] == 'page')
    page = client.send('Target.attachToTarget', {'targetId':target, 'flatten':True})['sessionId']
    tree = client.send('Accessibility.getFullAXTree', session=page)
    (evidence / 'full-ax.json').write_text(json.dumps(tree))
finally:
    client.close()
