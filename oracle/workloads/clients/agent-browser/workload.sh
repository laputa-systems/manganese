#!/bin/sh
set -eu
export AGENT_BROWSER_CDP=http://127.0.0.1:9221
agent-browser --json open http://forms.test/workload > /evidence/open.json
agent-browser --json fill '#text' 'agent-browser submission' > /evidence/fill.json
agent-browser --json click '#submit' > /evidence/submit.json
attempt=0
while [ ! -s /evidence/requests.jsonl ] && [ "$attempt" -lt 100 ]; do
    sleep 0.1
    attempt=$((attempt + 1))
done
test -s /evidence/requests.jsonl
agent-browser --json open http://forms.test/workload > /evidence/reopen.json
agent-browser --json snapshot > /evidence/snapshot.json
agent-browser --json eval 'navigator.userAgent' > /evidence/user-agent.json
agent-browser --json eval "document.querySelector('#same').contentDocument.querySelector('#same-text').textContent" > /evidence/same-frame.json
agent-browser --json screenshot /evidence/agent-browser.png > /evidence/screenshot.json
agent-browser --json close > /evidence/close.json
awk '$4 == "01" && ($2 ~ /:2405$/ || $3 ~ /:2405$/) { print $2, $3 }' /proc/net/tcp > /evidence/agent-sockets-before
cycle=0
while [ "$cycle" -lt 10 ]; do
    agent-browser --json open http://forms.test/workload > "/evidence/agent-cycle-$cycle-open.json"
    agent-browser --json close > "/evidence/agent-cycle-$cycle-close.json"
    cycle=$((cycle + 1))
done
awk '$4 == "01" && ($2 ~ /:2405$/ || $3 ~ /:2405$/) { print $2, $3 }' /proc/net/tcp > /evidence/agent-sockets-after
cmp /evidence/agent-sockets-before /evidence/agent-sockets-after
