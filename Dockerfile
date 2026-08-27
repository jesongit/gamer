# GameBot 一体化镜像（仓库根为构建上下文）：
#   stage1 node：corepack + pnpm --frozen-lockfile 安装依赖并 vite build 前端
#   stage2 rust：Cargo 清单预热依赖层（BuildKit cache mount：registry/git/target）
#   final debian-slim 运行时：内置 adb / ffmpeg / scrcpy-server.jar / 前端静态页
#
# 构建（必须在仓库根执行）：
#   docker build -t gamer .
# 运行见 docker-compose.yml；USB 直通见 docker-compose.usb.yml

# ---------- stage 1: 前端 ----------
FROM node:22-bookworm-slim AS web-builder
WORKDIR /web
# 先只拷清单与 pnpm 配置利用层缓存装依赖；
# pnpm-workspace.yaml 含 allowBuilds(esbuild)，必须先于 install 就位
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
RUN corepack enable \
    && pnpm install --frozen-lockfile
COPY web/index.html web/vite.config.js ./
COPY web/src ./src
# 仓库 vite 配置把产物输出到 ../server/web-dist（本机 dev 链路用），
# 镜像内改为输出到本地 dist，由 final 阶段取走
RUN pnpm build --outDir dist --emptyOutDir

# ---------- stage 2: Rust 服务端 ----------
FROM rust:1.97-slim AS rust-builder
WORKDIR /build
# 依赖预热层：只拷 Cargo 清单 + 假 main 先编译全部依赖，
# 清单不变时该层直接 CACHED；真实源码变化只会增量重编业务 crate
COPY server/Cargo.toml server/Cargo.lock ./
RUN mkdir -p src && printf 'fn main() {}\n' > src/main.rs
# 三处 cache mount：registry（crates 源包）/ git（git 依赖）/ target（增量重编译）
# 缓存卷跨构建持久，绕过 layer 缓存失效问题；产物须在同一 RUN 内拷出缓存挂载。
# 结尾必须清掉本 crate 的产物与指纹：否则下一段真实源码构建时，
# cargo 按 mtime 对比会误判“无需重编”而直接链接假 main 的空壳二进制
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release \
    && rm -rf target/release/.fingerprint/gamer-server* \
              target/release/deps/gamer_server* \
              target/release/gamer-server

COPY server/src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release && cp target/release/gamer-server /build/gamer-server

# ---------- final: 运行时 ----------
# 运行时底座必须与 builder 同代：rust:1.97-slim 现为 trixie 底（产物 glibc 符号
# 需要 GLIBC_2.39），bookworm 只有 2.36，会启动即 `GLIBC_2.39 not found`
FROM debian:trixie-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        adb \
        ffmpeg \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust-builder /build/gamer-server /usr/local/bin/gamer-server
COPY --from=web-builder /web/dist /app/web-dist
COPY server/assets/scrcpy-server.jar /app/assets/scrcpy-server.jar
# 服务端不再自建 data 目录（配置加载即校验路径），预建以便裸 docker run 可直接启动；
# compose 场景该目录被 ./server/data 绑定挂载覆盖
RUN mkdir -p /app/data
EXPOSE 8443
# 容器视角固定配置路径；镜像不内置 config.toml——缺省时代码默认值生效
# （端口 8443 / 数据目录 ./data→/app/data），自定义配置挂载到此路径即可
ENV GB_CONFIG=/app/config.toml
# 数据目录不声明 VOLUME 匿名卷：种子分区随部署方式注入（compose 绑定挂载
# ./server/data:/app/data），匿名卷会在镜像空目录之上遮蔽挂载内容
ENTRYPOINT ["gamer-server"]
