#!/bin/sh
set -eu
export AGENT_BROWSER_CDP=http://127.0.0.1:9221
export AGENT_BROWSER_NO_AUTO_DIALOG=true
agent-browser --json open http://forms.test/workload > /evidence/extra-open.json
agent-browser --json eval "({same:!!document.querySelector('#same'),cross:!!document.querySelector('#cross'),src:document.querySelector('#cross').src,ready:document.readyState})" > /evidence/frame-elements.json
agent-browser --json snapshot > /evidence/extra-snapshot.json
agent-browser snapshot > /evidence/extra-snapshot.txt
curl -fsS http://127.0.0.1:9221/json/list > /evidence/extra-targets.json
agent-browser --json frame '#same' > /evidence/same-frame-switch.json
agent-browser --json frame main > /evidence/main-frame-switch.json
reference=$(awk '/button "Cross-origin click"/ {sub(/^.*\[ref=/, ""); sub(/\].*$/, ""); print; exit}' /evidence/extra-snapshot.txt)
test -n "$reference"
agent-browser --json click "@$reference" > /evidence/cross-click.json
dd if=/dev/zero of=/profile/agent-upload.bin bs=150000 count=1 2>/dev/null
sha256sum /profile/agent-upload.bin > /evidence/agent-upload.sha256
agent-browser --json upload '#upload' /profile/agent-upload.bin > /evidence/upload.json
agent-browser --json click '#send-upload' > /evidence/send-upload.json
agent-browser --json eval "setTimeout(()=>document.querySelector('#confirm').click(),100);true" > /evidence/confirm-start.json
attempt=0
while [ "$attempt" -lt 50 ]; do
    agent-browser --json dialog status > /evidence/dialog-status.json
    case $(cat /evidence/dialog-status.json) in
        *'"hasDialog":true'*) break ;;
    esac
    sleep 0.1
    attempt=$((attempt + 1))
done
test "$attempt" -lt 50
agent-browser --json dialog accept > /evidence/dialog-accept.json
agent-browser --json eval 'document.body.dataset.confirm' > /evidence/confirm-result.json
agent-browser --json click '#popup' > /evidence/popup.json
agent-browser --json tab list > /evidence/popup-tabs.json
agent-browser --json tab close > /evidence/popup-close.json
agent-browser --json get count '#blob' > /evidence/download-selector.json
if agent-browser --json download '#blob' /downloads/client-download.txt > /evidence/download.json; then
    printf '%s\n' 0 > /evidence/download-exit
else
    printf '%s\n' "$?" > /evidence/download-exit
fi
agent-browser --json close > /evidence/extra-close.json
