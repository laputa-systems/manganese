FROM alpine@sha256:d56c381f961d307a21b3ca004cf1e3910f106644aefb1f43e654c8a56c4fd395
RUN apk add --no-cache curl
RUN curl -fsSL -o /usr/local/bin/agent-browser https://github.com/vercel-labs/agent-browser/releases/download/v0.43.0/agent-browser-linux-musl-x64 && echo '10b3b1d5fa8f53f7bc44273bcefaa64299ab77071bc83e75d96de5ed66025c89  /usr/local/bin/agent-browser' | sha256sum -c && chmod 755 /usr/local/bin/agent-browser
