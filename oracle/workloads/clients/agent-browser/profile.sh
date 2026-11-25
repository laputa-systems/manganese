#!/bin/sh
set -eu
export AGENT_BROWSER_CDP="${CDP_ENDPOINT:-http://127.0.0.1:9221}"
case "$1" in
    seed) url='http://forms.test/profile?seed' ;;
    read) url='http://forms.test/profile' ;;
    *) exit 2 ;;
esac
agent-browser --json open "$url" > "/evidence/agent-profile-$1-open.json"
agent-browser --json cookies get > "/evidence/agent-profile-$1-cookies.json"
agent-browser --json close > "/evidence/agent-profile-$1-close.json"
