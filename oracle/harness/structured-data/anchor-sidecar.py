import json
import pathlib
import sys

sys.path.insert(0, '/launch')
from cdp import Cdp

cases = json.loads(pathlib.Path(sys.argv[1]).read_text())
source = '''(async () => {
    const results = [];
    for (const [kind, text] of INPUTS) {
        if (kind === 'json') {
            results.push(JSON.stringify(JSON.parse(text)));
        } else {
            const doc = new DOMParser().parseFromString(text, 'application/xml');
            results.push(new XMLSerializer().serializeToString(doc));
        }
    }
    return results;
})()'''

client = Cdp.connect_when_ready()
try:
    page = client.page()
    client.send('Fetch.enable', {'patterns': [{'urlPattern': 'https://structured.test/*'}]}, session=page)
    client.evaluate(page, "location.href='https://structured.test/'; 'navigating'")
    event = client.wait_event('Fetch.requestPaused', session=page)
    client.send('Fetch.fulfillRequest', {'requestId': event['requestId'], 'responseCode': 200,
                                        'responseHeaders': [{'name': 'Content-Type', 'value': 'text/html'}],
                                        'body': 'PGh0bWw+PHRpdGxlPnN0cnVjdHVyZWQ8L3RpdGxlPjwvaHRtbD4='}, session=page)
    client.send('Fetch.disable', session=page)
    results = []
    for start in range(0, len(cases), 8):
        results.extend(client.evaluate(page, source.replace('INPUTS', json.dumps(cases[start:start + 8])), timeout=120))
    print(json.dumps(results))
finally:
    client.close()
