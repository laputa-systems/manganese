FROM python@sha256:191083cdf6c7bda8c76158983163ead5d5cf7b77cf3033e805bb0f5296f51b4b
RUN apt-get update && apt-get install -y --no-install-recommends git && rm -rf /var/lib/apt/lists/*
RUN python -m pip install --no-cache-dir 'git+https://github.com/browser-use/browser-use@070be98109c03f04a95f704533e8f6e9868d0525'
