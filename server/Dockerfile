# GameBot 服务端 Docker 镜像
# 多阶段构建：rust builder → slim 运行时（内置 adb + ffmpeg）

FROM rust:1.97-slim AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    adb \
    ffmpeg \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /build/target/release/gamer-server /usr/local/bin/gamer-server
COPY assets/scrcpy-server.jar /app/assets/scrcpy-server.jar

# 数据目录（SQLite、模板、脚本）
VOLUME ["/app/data"]
EXPOSE 8443

ENV GB_CONFIG=/app/config.toml
ENTRYPOINT ["gamer-server"]
