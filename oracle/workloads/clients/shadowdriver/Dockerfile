FROM python@sha256:0f9552bb28711333da16915ec146f5e6bb44a7fdac9ced15dc17c2e0206346dd
RUN apk add --no-cache build-base curl linux-headers openssl-dev pkgconf
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain nightly-2026-11-09
WORKDIR /src
COPY . /src
RUN python -m pip install --no-cache-dir ./py && python -m pip install --no-cache-dir ./grab
