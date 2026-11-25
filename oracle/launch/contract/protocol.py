import json
import urllib.request

result = {'completed':0, 'browser_answers':False}
try:
    for number in range(1, 101):
        result['request_number'] = number
        with urllib.request.urlopen('http://127.0.0.1:9221/json/protocol', timeout=10) as response:
            json.load(response)
        result['completed'] += 1
    with urllib.request.urlopen('http://127.0.0.1:9221/json/version', timeout=10) as response:
        result['browser_answers'] = response.status == 200
except Exception as error:
    result['error'] = repr(error)
print(json.dumps(result))
