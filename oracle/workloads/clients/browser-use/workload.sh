#!/bin/sh
set -eu
export BU_CDP_URL=http://127.0.0.1:9221
export BROWSER_USE_TELEMETRY=false
browser-use > /evidence/browser-use.txt <<'PY'
import json
print(json.dumps({'version':cdp('Browser.getVersion'), 'tabs':list_tabs()}))
goto_url('http://forms.test/workload')
wait_for_load()
print(json.dumps({'page':page_info(), 'user_agent':js('navigator.userAgent'),
                  'ax':cdp('Accessibility.getFullAXTree')}))
print(capture_screenshot('/evidence/browser-use.png'))
fill_input('#text', 'browser-use submission')
rect = js("(()=>{let r=document.querySelector('#submit').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()")
click_at_xy(rect['x'], rect['y'])
PY
