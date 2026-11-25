import json
import sys
sys.path.insert(0, '/launch')
from cdp import Cdp

client = Cdp.connect()
client.send('Target.setDiscoverTargets', {'discover': True})
target = client.send('Target.createTarget', {'url':'about:blank'})['targetId']
session = client.send('Target.attachToTarget', {'targetId':target, 'flatten':True})['sessionId']
client.socket.send_text(json.dumps({'id':99999,'method':'Page.crash','sessionId':session,'params':{}}))
event = client.wait_event('Target.targetCrashed', timeout=30)
client.socket.send_text(json.dumps({'id':100000,'method':'Browser.crash','params':{}}))
try:
    client.wait_event('this.event.does.not.exist', timeout=15)
except Exception:
    pass
print(json.dumps({'target':target, 'crashed_target':event.get('targetId'), 'browser_crash_sent':True}))
