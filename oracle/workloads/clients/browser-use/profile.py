import json
import os

url = 'http://forms.test/profile?seed' if os.environ['PROFILE_MODE'] == 'seed' else 'http://forms.test/profile'
goto_url(url)
wait_for_load()
print(json.dumps(cdp('Network.getCookies', urls=['http://forms.test/'])))
