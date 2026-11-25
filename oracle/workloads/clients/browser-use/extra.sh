#!/bin/sh
set -eu
export BU_CDP_URL=http://127.0.0.1:9221
export BROWSER_USE_TELEMETRY=false
dd if=/dev/zero of=/profile/browser-use-upload.bin bs=150000 count=1 2>/dev/null
sha256sum /profile/browser-use-upload.bin > /evidence/browser-use-upload.sha256
browser-use < /workload/extra.py > /evidence/browser-use-extra.txt
