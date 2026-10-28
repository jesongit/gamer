# 0.5.8 依赖安全升级证据（2026-10-29）

本次发行前预检确认旧服务端锁文件有 35 项 RustSec 漏洞报告及维护/健全性警告，用户选择先升级依赖再发行。补查 npm 官方漏洞源后，Web 锁文件也有 35 项报告；镜像不携带 Node/Vite，但运行时代码使用的 Vue、js-yaml 与构建测试依赖一并升级。

## 依赖与补丁

| 范围 | 当前锁定版本/处理 |
| --- | --- |
| Tokio / Wasmtime | 1.54.1 / 48.0.7 |
| WebRTC / DTLS | webrtc 0.17.2；保留 Tokio API，dtls 0.17.1 使用可追溯源码补丁 |
| RustCrypto | P-256、P-384 0.14；crypto-bigint 0.7.10 |
| TLS | rustls 0.23.46、rustls-webpki 0.103.16、quinn-proto 0.11.19 |
| YAML | 服务端与映射 guest 用 serde_norway 0.9.42，移除 serde_yaml / unsafe-libyaml |
| ZIP | 宿主、更新器及独立打包器用 zip 9.1.3；保留官方发行工具使用的 Store/Deflate，移除不用的 Zstd 等后端 |
| 前端运行库 | Vue 3.5.44、js-yaml 5.5.0 |
| 前端构建/测试 | Vite 7.3.7、plugin-vue 6.0.10、Vitest 4.1.12、happy-dom 21.0.4 |

DTLS 上游发布归档 SHA256 为 `2f016db07b91e9d79cc60a152c163d3f0ce2d4c0173cb3964de3526aab6e07fa`，源码提交为 `e9fe1bc4207a0c6b8d28d5583a118419be60f3c2`，许可与补丁记录保留在 `third_party/dtls/`。仅适配官方 RustCrypto 公钥点和安全随机数 API，不实现自定义密码学运算；P-256/P-384 增加共享密钥一致性、未压缩公钥格式与非法点拒绝回归。

## 审计与回归

- `tools/verify-release.ps1` 使用 cargo-audit 0.22.2，六份活动 Rust 锁文件全部严格通过：服务端、更新器、打包器、映射 guest、YAML guest、唯一 YAML 解释器。警告视为失败，包含撤回包检查，移除旧 bincode 豁免，无 `--ignore`。
- RustSec 数据库本轮快照 `40f5be6fb12ed93fc591bf4ab23b7d793b3f2239`，载入 1381 条公告；此结论是检查时的已知公告结果。
- Web 与六款活动插件的七份前端锁文件，使用 npm 官方源 `pnpm audit --registry=https://registry.npmjs.org --audit-level=low`，各级报告数均为零。国内镜像的审计接口返回 405，不能当作通过，因此审计显式使用官方源。
- 服务端 1203 通过 / 54 忽略；DTLS 61 通过；更新器 94 通过；打包器 4 通过。宿主 Clippy `-D warnings` 与无 WASM 构建通过。
- Web 885 通过，插件 UI 共 189 通过；生产 Web 与插件 UI 构建通过。DOM 确认框桩显式设置；Windows 拒绝访问测试在服务端源码所在盘创建独立合成私密文件，防止跨盘缺失文件的 SPA 回退掩盖访问控制断言，仍要求 HTTP 403 且不返回私密标记。
- 主仓 CI 增加 Rust/前端安全门禁和 DTLS 回归；插件 CI 增加六份 UI 审计及宿主/guest 安全门禁。SDK 固定到已公开提交并验证逐文件哈希。

全部插件 UI 重建为新版本，旧归档和候选标签不覆盖。活动发行版本为 YAML 0.3.27、视频 0.2.9、映射 0.1.9、直播 0.2.20、通知 0.1.3、配置包发布 0.1.1。退休 AI/OCR 安装与私密数据不清理，不重新分发。

本文记录源码与本地检查。Docker 公开摘要、公开附件哈希和隔离跨容器验收另见对应发行回执；不代表真实用户设备、游戏账号、退出素材或脚本超时退出已验收，也不将 Rust/npm 审计等同于操作系统完整漏洞扫描。

## 上游依据

- [RustCrypto 时序问题与修复版本](https://rustsec.org/advisories/RUSTSEC-2026-0387.html)
- [DTLS 0.17.1 官方归档](https://crates.io/api/v1/crates/dtls/0.17.1/download)
- npm 官方公告由审计返回的 GHSA 标识关联，发行附件记录各锁文件 SHA256、依赖清单与审计元数据。
