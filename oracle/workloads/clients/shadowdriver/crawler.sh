#!/bin/sh
set -eu
ALL_PROXY=socks5h://127.0.0.1:1080 grab --control-url "${CDP_ENDPOINT:-http://127.0.0.1:9221}" --single --no-sitemap --no-ublock --no-stealth --out /evidence/grab http://forms.test/workload
test -s /evidence/grab/forms.test/workload/index.html
