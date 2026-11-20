# The Rust stack in one Dockerfile, one target per runtime image. Build context: rust-version/
#
#   docker build --target api      -t granary-rust-api      rust-version   # backend, worker, beat
#   docker build --target pdf      -t granary-rust-pdf      rust-version
#   docker build --target face     -t granary-rust-face     rust-version
#   docker build --target signal   -t granary-rust-signal   rust-version
#   docker build --target research -t granary-rust-research rust-version
#
# Seven binaries come out of ONE cargo build (stage `build`), so Compose building the seven services
# compiles the workspace once (granary-face has its own small build on Debian trixie, stage `build-face`): BuildKit
# shares the stage between targets, and the cargo cache mounts are `sharing=locked`, so builds that do run
# side by side wait for each other and then find everything fresh.
# Dependencies stay compiled in the cache mounts (cargo registry + target/) between builds: a `git pull`
# recompiles only the crates that changed. `docker builder prune` drops that cache (the next build is a full one).
#
# Same ports, env names and volumes as the Python/Node images they replace (see the root docker-compose.yml,
# which builds these targets under the fixed names granary-rust-*:latest).

# ------------------------------------------------------------------------------------------------------------
# 1. Compile every binary.
FROM rust:1.99-bookworm AS build
# CARGO_BUILD_JOBS: parallel compile jobs; the default suits a host with about 8 GB of RAM, raise it on a bigger
# one (`--build-arg CARGO_BUILD_JOBS=4`).
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS} \
    CARGO_PROFILE_RELEASE_STRIP=symbols \
    CARGO_TERM_COLOR=never \
    CARGO_TERM_PROGRESS_WHEN=never
WORKDIR /src
COPY . .
RUN --mount=type=cache,id=granary-rust-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=granary-rust-target,target=/src/target,sharing=locked \
    cargo build --release --locked \
        -p granary-api -p granary-worker -p granary-db -p granary-pdf -p granary-signal -p granary-research --bins \
    && mkdir -p /out \
    && cd target/release \
    && cp granary-api granary-worker granary-migrate granary-seed granary-pdf granary-signal granary-research /out/

# 1b. granary-face on Debian trixie: ort links the prebuilt ONNX Runtime 1.31 static library (downloaded by its
#     build script from cdn.pyke.io), which needs glibc 2.38+ and GCC 13's libstdc++; bookworm has 2.36 / GCC 12.
#     libturbojpeg0: granary-face links libturbojpeg.so.0 (src/decode.rs).
FROM rust:1.99-trixie AS build-face
RUN apt-get update && apt-get install -y --no-install-recommends libturbojpeg0 \
    && rm -rf /var/lib/apt/lists/*
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS} \
    CARGO_PROFILE_RELEASE_STRIP=symbols \
    CARGO_TERM_COLOR=never \
    CARGO_TERM_PROGRESS_WHEN=never
WORKDIR /src
COPY . .
RUN --mount=type=cache,id=granary-rust-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=granary-rust-face-target,target=/src/target,sharing=locked \
    cargo build --release --locked -p granary-face --bins \
    && mkdir -p /out && cp target/release/granary-face /out/

# ------------------------------------------------------------------------------------------------------------
# 2. face-api.js 1.7.15 (the npm tarball, checksum-pinned): the browser bundle and tfjs weights that GET /assets
#    serves, and the weights the ONNX models are converted from (services/face/models/convert.py, deterministic:
#    the outputs are checked against the hashes in services/face/models/README.md).
FROM python:3.12-slim AS face-models
ARG FACE_API_SHA256=0d77fffeef362ef9c0939cf6c567f3a2e8e177f114fa257a3270cbd2e509b031
RUN pip install --no-cache-dir onnx==1.23.2 numpy==2.1.3
RUN python -c "import urllib.request; urllib.request.urlretrieve('https://registry.npmjs.org/@vladmandic/face-api/-/face-api-1.7.15.tgz', '/tmp/face-api.tgz')" \
    && echo "${FACE_API_SHA256}  /tmp/face-api.tgz" | sha256sum -c - \
    && mkdir -p /pkg /out/face-api/dist \
    && tar -xzf /tmp/face-api.tgz -C /pkg \
    && cp /pkg/package/dist/face-api.esm.js /out/face-api/dist/ \
    && cp -r /pkg/package/model /out/face-api/model \
    && rm -f /tmp/face-api.tgz
COPY services/face/models/convert.py /convert.py
RUN python /convert.py /out/face-api/model /out/onnx \
    && cd /out/onnx \
    && printf '%s  %s\n' \
        631207b1b0a83b639820385924c6462ddf8c77a3dcad0415543393fc73341b14 ssd_mobilenetv1.onnx \
        75924e2f20fd34e7da4d5e4d9b610d6999505d23a4d63f143061b433b7913b09 face_landmark_68.onnx \
        3dfd1770afce3ee30c1928c9c49c625e60a8cb78775caea68f01503764ed106e face_recognition.onnx \
       | sha256sum -c -

# ------------------------------------------------------------------------------------------------------------
# 3. Runtime images: Debian bookworm (the base of the node:20-slim images and of the Python images as built so
#    far), face on trixie (see 1b).

# API + jobs: backend (granary-migrate && granary-seed && granary-api), worker (granary-worker worker), beat (granary-worker beat).
# curl for the Compose healthcheck; ca-certificates for SMTP/IMAP/HTTPS (rustls-native-certs reads the system
# store, as Python's ssl.create_default_context does). Runs as root like the Python image: it writes the PAdES
# seal certificate into the existing root-owned signing_keys volume (/keys).
FROM debian:bookworm-slim AS api
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/granary-api /out/granary-worker /out/granary-migrate /out/granary-seed /usr/local/bin/
WORKDIR /app
EXPOSE 8000
CMD ["sh", "-c", "granary-migrate && granary-seed && exec granary-api"]

# PDF: the same Chromium package and fonts the node:20-slim image installed, so documents render the same.
# tini reaps the Chromium helpers left behind when a render is killed at its time limit.
FROM debian:bookworm-slim AS pdf
RUN apt-get update && apt-get install -y --no-install-recommends \
        chromium fonts-liberation fonts-noto-cjk ca-certificates tini \
    && rm -rf /var/lib/apt/lists/*
# PUPPETEER_EXECUTABLE_PATH keeps the Node service's variable name for the Chromium binary.
ENV PUPPETEER_EXECUTABLE_PATH=/usr/bin/chromium \
    PDF_ASSETS_DIR=/app/assets \
    PORT=3002
WORKDIR /app
COPY --from=build /out/granary-pdf /usr/local/bin/granary-pdf
# The document templates.
COPY services/pdf/assets/ /app/assets/
EXPOSE 3002
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["granary-pdf"]

# Face: libturbojpeg0 (JPEG decoding bit-identical to TensorFlow's), libstdc++6 (ONNX Runtime, linked in).
FROM debian:trixie-slim AS face
RUN apt-get update && apt-get install -y --no-install-recommends libturbojpeg0 libstdc++6 ca-certificates \
    && rm -rf /var/lib/apt/lists/*
ENV FACE_API_DIR=/app/face-api \
    FACE_ONNX_DIR=/app/onnx \
    FACE_ANTISPOOF_DIR=/app/antispoof \
    PORT=3003
WORKDIR /app
COPY --from=build-face /out/granary-face /usr/local/bin/granary-face
COPY --from=face-models /out/face-api /app/face-api
COPY --from=face-models /out/onnx /app/onnx
# The passive check's MiniFASNet anti-spoof models: committed (models/antispoof/README.md), not generated.
COPY services/face/models/antispoof/minifasnet_v2_2.7.onnx services/face/models/antispoof/minifasnet_v1se_4.0.onnx /app/antispoof/
EXPOSE 3003
CMD ["granary-face"]

# Signal: wget keeps the Compose healthcheck (`wget -qO- http://localhost:3003/health`) working unchanged.
FROM debian:bookworm-slim AS signal
RUN apt-get update && apt-get install -y --no-install-recommends wget \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/granary-signal /usr/local/bin/granary-signal
ENV PORT=3003
EXPOSE 3003
CMD ["granary-signal"]

# Research: Moli (a headless browser in one binary), pinned and checksum-verified, the same release as the Python
# image; bump both together, after checking the new release on a few real company sites. ca-certificates and
# libfontconfig1 are for Moli; the Rust binary carries its own TLS roots (webpki-roots).
FROM debian:bookworm-slim AS research
ARG MOLI_VERSION=1.1.9
ARG MOLI_SHA256=34d00b75a6ca59119ac9ec09e6fc0c9ee264c215e9c482440d159fa63a589ba5
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl libfontconfig1 \
    && curl -fsSL -o /tmp/moli.tgz \
        "https://github.com/lexmount/moli/releases/download/v${MOLI_VERSION}/moli-x86_64-unknown-linux-gnu.tar.gz" \
    && echo "${MOLI_SHA256}  /tmp/moli.tgz" | sha256sum -c - \
    && tar -xzf /tmp/moli.tgz -C /tmp \
    && install -m 0755 "/tmp/moli-v${MOLI_VERSION}-x86_64-unknown-linux-gnu/moli" /usr/local/bin/moli \
    && rm -rf /tmp/moli* \
    && apt-get purge -y curl && apt-get autoremove -y \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/granary-research /usr/local/bin/granary-research
# Nothing here needs root.
RUN useradd --system --home /tmp research
USER research
ENV HOST=0.0.0.0 \
    PORT=3004
EXPOSE 3004
CMD ["granary-research"]
