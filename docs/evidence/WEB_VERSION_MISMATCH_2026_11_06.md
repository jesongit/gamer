# 前后端版本不一致修复（2026-11-06）

用户报告前端 0.5.16 / 服务端 0.5.17。检查确认当前主仓 web/package.json 为 0.5.16，server/Cargo.toml 为 0.5.17；既有 tools/check-version.ps1 同时检出 updater 及其两份 Cargo.lock 仍为 0.5.13，共四项不一致。

当天宿主热修部署记录（SCHEDULE_FIX_2026_11_06.md）明确仅替换服务端二进制、继承 v1.0.16 基础镜像的前端；忽略目录中保留的 Dockerfile.hotfix 与该记录一致。上述证据解释了告警来源，本轮未读取线上 HTTP 资源，未声称已验收线上版本。

## 源码修复

- 前端、更新器及对应锁文件同步到服务端权威版本 0.5.17。
- Vite 在开发启动和生产构建前校验前端元数据与 server/Cargo.toml 的 package.version，版本不一致或权威版本缺失立即失败；仍注入真实前端构建版本，保留运行时混包告警。
- Docker 前端构建阶段显式复制 server/Cargo.toml，让容器构建执行同一门禁。门禁不检查继承镜像中的旧产物，热修仍必须同时交付经核对的 web-dist。

## 本地验证

- tools/check-version.ps1 通过，产品版本 0.5.17。
- 版本门禁与既有告警回归：2 文件 / 8 测试通过，覆盖旧前端与新服务端组合拒绝、预发布版本、CRLF/注释和缺少产品版本时不误取依赖版本。
- 前端生产构建通过，输出到隔离目录 server/target/web-version-fix-20261106；MainLayout 构建产物中的前端版本为 0.5.17。
- server 和 updater 的 cargo metadata --locked --offline --no-deps 通过；git diff --check 通过。

本轮仅修改源码、构建隔离前端产物并执行本地验证，未构建完整容器镜像、未发布、未更新线上安装或重启服务。线上修复须将本次前端与匹配的服务端一起交付，之后刷新页面并复核实际 HTTP 资源与 /api/system/info；不能把本地构建通过视为线上告警已消除。
