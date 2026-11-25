#!/bin/sh
set -eu
export BU_CDP_URL="${CDP_ENDPOINT:-http://127.0.0.1:9221}"
export BROWSER_USE_TELEMETRY=false
export PROFILE_MODE="$1"
browser-use < /workload/profile.py > "/evidence/browser-use-profile-$1.json"
