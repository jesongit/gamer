# syntax=docker/dockerfile:1
ARG RUST_IMAGE=rust:1.98
ARG NODE_IMAGE=node:22-bookworm-slim
ARG RUNTIME_IMAGE=debian:trixie-slim

FROM --platform=$BUILDPLATFORM ${NODE_IMAGE} AS web-build
WORKDIR /src
RUN corepack enable && corepack prepare pnpm@11.23.0 --activate
COPY sdk ./sdk
COPY plugins ./plugins
COPY tools/build-plugin-ui.mjs ./tools/build-plugin-ui.mjs
COPY web ./web
RUN pnpm --dir web install --frozen-lockfile \
    && node tools/build-plugin-ui.mjs --install-only \
    && node tools/build-plugin-ui.mjs \
    && pnpm --dir web build

FROM ${RUST_IMAGE} AS server-build
ARG TARGETARCH
ARG GAMER_GIT_COMMIT=unknown
ARG GAMER_BUILD_TIME=unknown
ENV CARGO_BUILD_JOBS=1 GAMER_CHANNEL=stable
WORKDIR /src
COPY server/Cargo.toml server/Cargo.lock server/build.rs ./server/
COPY server/src ./server/src
COPY server/wit ./server/wit
COPY third_party ./third_party
COPY docs/reference/YAML.md ./docs/reference/YAML.md
COPY server/assets/scrcpy-server.jar ./server/assets/scrcpy-server.jar
COPY plugins ./plugins
COPY sdk ./sdk
COPY updater ./updater
COPY release/config.default.toml ./release/config.default.toml
COPY release/dependencies.lock.toml ./release/dependencies.lock.toml
COPY release/runtime ./release/runtime
RUN --mount=type=cache,id=gamer-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=gamer-target-${TARGETARCH},target=/src/server/target,sharing=locked \
    cargo build --release --locked --manifest-path server/Cargo.toml \
    && mkdir -p /out && cp server/target/release/gamer-server /out/gamer-server

FROM ${RUNTIME_IMAGE} AS runtime-base
RUN apt-get -o Acquire::Retries=3 update && apt-get -o Acquire::Retries=3 install -y --no-install-recommends \
      ca-certificates chromium chromium-sandbox ffmpeg adb \
      tesseract-ocr tesseract-ocr-eng tesseract-ocr-chi-sim \
      fonts-noto-cjk tzdata python3 python3-cryptography util-linux \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1000 gamer && useradd --uid 1000 --gid gamer --create-home gamer

FROM runtime-base AS runtime
RUN apt-get -o Acquire::Retries=3 update && apt-get -o Acquire::Retries=3 install -y --no-install-recommends python3-websocket python3-argon2 gh \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=server-build /out/gamer-server /app/gamer-server
COPY --from=web-build /src/server/web-dist /app/web-dist
COPY server/assets/scrcpy-server.jar /app/assets/scrcpy-server.jar
COPY deploy/docker/config.toml /app/config.default.toml
COPY deploy/docker/entrypoint.py deploy/docker/healthcheck.py /app/
COPY deploy/docker/chromium-seccomp.json /app/chromium-seccomp.json
COPY deploy/docker/restore-volume.py deploy/docker/volume-transfer.py /app/tools/
COPY tools/gamer_docker.py tools/gamer_browser_sessions.py /app/tools/
ENV GAMER_PROFILE=prod GAMER_APP_DIR=/app GAMER_DATA_DIR=/data \
    GB_CONFIG=/data/config.toml GB_LOG=stdout TZ=Asia/Shanghai \
    TESSDATA_PREFIX=/usr/share/tesseract-ocr/5/tessdata \
    GAMER_SECRETS_KEY_FILE=/data/state/credentials.key ADB_MDNS=0 \
    TOKIO_WORKER_THREADS=2 MALLOC_ARENA_MAX=2
ARG GAMER_GIT_COMMIT=unknown
ARG GAMER_BUILD_TIME=unknown
ARG GAMER_RELEASE_VERSION=dev
LABEL org.opencontainers.image.title="Gamer" \
      org.opencontainers.image.source="https://github.com/jesongit/gamer" \
      org.opencontainers.image.revision="${GAMER_GIT_COMMIT}" \
      org.opencontainers.image.created="${GAMER_BUILD_TIME}" \
      org.opencontainers.image.version="${GAMER_RELEASE_VERSION}"
EXPOSE 8443
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=12s --start-period=60s --retries=3 \
    CMD ["python3", "/app/healthcheck.py"]
ENTRYPOINT ["python3", "/app/entrypoint.py"]
CMD ["/app/gamer-server"]
