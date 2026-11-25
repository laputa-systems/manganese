import os
import json
import sys
import time

sys.path.insert(0, '/launch')
from cdp import Cdp

client = Cdp.connect(os.environ.get('CDP_ENDPOINT', 'http://127.0.0.1:9221'))
extension_id = client.send('Extensions.loadUnpacked', {'path':'/profile/extension'})['id']
extensions = client.send('Extensions.getExtensions')['extensions']
target = client.send('Target.createTarget', {'url':'http://forms.test/workload'})['targetId']
time.sleep(1)
client.send('Target.setDiscoverTargets', {'discover':True, 'filter':[{'type':'tab'}]})
targets = client.send('Target.getTargets')['targetInfos']
tab = next(item['targetId'] for item in targets if item['type'] == 'tab' and item['url'] == 'http://forms.test/workload')
action = client.send('Extensions.triggerAction', {'id':extension_id, 'targetId':tab})
print(json.dumps({'id':extension_id, 'extensions':extensions, 'target':target, 'tab':tab, 'action':action, 'targets':targets}))
client.close()
