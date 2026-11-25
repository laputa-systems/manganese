#!/bin/sh
set -eu
export BU_CDP_URL="${CDP_ENDPOINT:-http://127.0.0.1:9221}"
export BROWSER_USE_TELEMETRY=false
awk '$4 == "01" && ($2 ~ /:2405$/ || $3 ~ /:2405$/) { print $2, $3 }' /proc/net/tcp > /evidence/browser-use-sockets-before
cycle=0
while [ "$cycle" -lt 10 ]; do
    browser-use < /workload/version.py > "/evidence/browser-use-cycle-$cycle.json"
    browser-use --reload > "/evidence/browser-use-reload-$cycle.txt"
    cycle=$((cycle + 1))
done
attempt=0
while [ "$attempt" -lt 50 ]; do
    awk '$4 == "01" && ($2 ~ /:2405$/ || $3 ~ /:2405$/) { print $2, $3 }' /proc/net/tcp > /evidence/browser-use-sockets-after
    if [ ! -s /evidence/browser-use-sockets-after ]; then
        break
    fi
    sleep 0.1
    attempt=$((attempt + 1))
done
test ! -s /evidence/browser-use-sockets-after
