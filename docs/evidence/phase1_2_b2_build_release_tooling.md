# Phase 1 §4.2/§4.3 证据：构建与发布工具 + 官方 GitHub 市场服务端无关部分（Agent B2）

> 计划：`docs/plans/gamer_plugin_ecosystem_video_finalization_plan.md` §4.2、§4.3（服务端无关部分）
> 日期：2026-09-07
> 所有权范围：`tools/build-plugins.ps1`、`tools/plugin-signer/**`、`.github/workflows/**`（按需）、`web/public/registry.json` + `web/public/plugins/*.gplugin`、`tools/plugin-signing/**`。未改 `server/**`、`web/src/**`、`tools/plugins/**`（manifest v2 由并行 Agent B1 负责，本轮中途已落树，最终产物基于其内容生成）。

## 1. 完成项

1. **plugin-signer 与签名解耦**（`tools/plugin-signer` v0.2.0）：
   - `pack --manifest <toml> [--wasm <f>] --out <gplugin> [--file a=b]... [--meta-out <json>]`：不再要求 `--key`/`--key-id`，不写 `signature.sig`；`execution.kind=wasm` 必须提供 `--wasm` 且 manifest 声明 `entry`，`kind=builtin` 禁止 `--wasm`（防止占位/伪造 WASM 进入正式包）；对输入 wasm 预检 `\0asm` magic。
   - 新增 `inspect`（只解析 manifest 元数据）与 `verify --archive`（产物自检：重走 zip 中央目录、manifest 可解析、wasm 包 entry 存在且 magic 正确、builtin 跳过 entry 校验；打印 id/version/kind/sha256/size）。
   - 新增 manifest v2 TOML 解析（`toml` crate，本地 cargo 缓存已有 0.8.23，无新增网络依赖）：元数据（id/version/name/description/publisher/entry/execution.kind/builtin_id/permissions/host_api/ui.contributions）以 manifest 为唯一权威源，`--meta-out` 输出 JSON 供构建脚本消费（registry 元数据收敛，消除 ps1 硬编码双份维护）。
   - legacy 保留：`keygen`/`sign`（旧「打包+Ed25519 签名」一体路径）仅在应急时手工调用，默认链不调用；`registry-proof` 子命令随 Registry proof 机制删除。
2. **build-plugins.ps1 重写**（免签名默认链）：构建 signer → 枚举 `<ManifestsRoot>/*/manifest.toml` 并 `inspect` 解析 → 仅 wasm 包构建 guest Component（keymap=server/tests/keymap-guest、yaml=server/guests/yaml-guest；builtin 的 video 无 guest 无占位 wasm）→ pack 落 **staging 临时目录** → 自检（`signer verify` + PowerShell 独立重算 sha256/size + id/version/kind 与 manifest 比对）→ 全部通过后才：写 registry v2 临时文件 → 产物拷入 OutputDir（拷贝后复核 sha256）→ **原子替换** registry（`Move-Item -Force`）→ 清理不在本轮清单内的旧 `.gplugin`（`-KeepStaleArtifacts` 跳过）。任何失败 exit 非零、staging/临时文件清理、既有产物零触碰。
   - 参数：`-OutputDir`（默认 web/public/plugins）、`-RegistryFile`（默认 web/public/registry.json）、`-ManifestsRoot`（默认 tools/plugins，fixture 干跑用）、`-ChecksumsFile`、`-Publisher`（manifest 无 publisher 字段时的兜底，默认 gamer.dev）、`-KeepStaleArtifacts`。
   - registry v2：`schema_version=2`；条目 = id/version/name/description/publisher/download_url/sha256/size/permissions/host_api/ui + `execution{kind}`；**无 signature 字段**；顶层保留 generated_at/host_api。
3. **GitHub Release 脚本化**（§4.2 后两条）：`-ChecksumsFile` 生成 `sha256sums.txt`（GNU `sha256sum -c` 兼容；头注释含 `# source_commit: <git rev-parse HEAD>` 与 generated_at；覆盖全部 `.gplugin` + `registry.json`），配合 `-OutputDir` 即「从指定提交构建 → 完整性清单 → 产物可上传」。不要求真发 Release；本地产物可直接导入，web/public 继续作开发 seed。
4. **tools/plugin-signing 目录整体删除**：未入库的私钥 `gamer-dev-1.key` 已删除；写证据文档期间服务端 verifier（`server/src/extensions/signature.rs`，含 `include_str!` pem 的同步锁测试）由并行 Agent 落地删除，对 pem 的最后引用消失，遂将 `tools/plugin-signing/`（README + gamer-dev-1.pem）整目录删除。signer 的 `keygen`/`sign` 子命令仅为存量签名包应急保留，默认链不调用。
5. **CI 无需改动**：`.github/workflows/ci.yml`、`tools/ci-local.ps1` 均无 plugin-signing/keygen/registry-proof 引用（grep 证实）；`release.yml` 的签名是 launcher/主程序更新清单签名（`RELEASE_MANIFEST_PRIVATE_KEY`），按基线 §6 明确不在删除范围，未触碰。CI 不存任何插件私钥。

## 2. 修改文件

| 文件 | 变更 |
| --- | --- |
| `tools/plugin-signer/Cargo.toml` | v0.2.0；+`toml = "0.8"`；描述更新（Cargo.lock 随之更新） |
| `tools/plugin-signer/src/main.rs` | 重写：pack/inspect/verify 默认链 + legacy keygen/sign；删 registry-proof |
| `tools/build-plugins.ps1` | 重写为免签名链（UTF-8 带 BOM；PS 5.1 兼容） |
| `tools/plugin-signing/` | 整目录删除（README + pem；`.key` 本就未入库） |
| `web/public/registry.json` | 重新生成为 schema v2、3 条目、无 signature |
| `web/public/plugins/*.gplugin` | `gamer.keymap-1.0.1.gplugin`、`gamer.video-1.0.0.gplugin`、`gamer.yaml-3.1.1.gplugin`（无签名）；旧 `-1.0.0`/`-3.1.0` 已清理 |
| `docs/PITFALLS.md` | 追加 2 条 PS 5.1 坑（BOM、pscustomobject 属性） |

## 3. registry v2 样例条目（实取自 web/public/registry.json，简化缩进）

```json
{
  "id": "gamer.video",
  "version": "1.0.0",
  "name": "视频工作台",
  "description": "视频工作台：媒体素材库、设备录制与操作草稿生成",
  "publisher": "gamer.dev",
  "download_url": "/plugins/gamer.video-1.0.0.gplugin",
  "sha256": "c1329601d91a…（64hex）",
  "size": 692,
  "permissions": ["media.read", "media.import", "media.record", "media.write", "media.events.read"],
  "host_api": {},
  "ui": { "contributions": [ { "panel_id": "video", "title": "视频", "icon": "🎬", "order": 28,
                               "location": "console.right", "runtime": "core",
                               "requires_device": false, "preferred_width": 440,
                               "component": "VideoWorkbench" } ] },
  "execution": { "kind": "builtin" }
}
```

wasm 条目（keymap/yaml）同构，`execution.kind="wasm"`、`host_api` 为各域 `^1.0` 表。包内容（`zipfile` 实测）：wasm 包 = `manifest.toml + plugin.wasm`；video 包 = 仅 `manifest.toml`；所有包**无 `signature.sig`**。

## 4. 测试 / 干跑证据（全部实跑）

**signer 单测（临时 fixture，`/tmp/b2fx`）**
- 正例：`inspect`（wasm/builtin，`--meta-out` JSON 字段齐全，TOML 字符串含 `#`/中文/emoji 解析正确）；`pack` wasm 与 builtin（无 `--wasm`）成功且输出 sha256/size；`verify` 对两种包 id/version/kind/sha256/size 与 pack 输出一致；v1 无 `[execution]` 的 manifest 推断 `kind=wasm`（向后兼容读取）。
- 反例（全部拒绝、exit 非零）：builtin 带 `--wasm`；wasm 缺 `--wasm`；`--wasm` 非 wasm 字节；wasm manifest 缺 `entry`；`kind="native"` 非法；manifest 缺 `name`；verify 对损坏 zip（EOCD 缺失）、缺 entry 成员、entry 坏 magic 全部拒绝。
- clippy：`cargo clippy -- -D warnings` PASS；`cargo fmt --check` PASS。

**build-plugins.ps1 负例（注册表保护）**
- 构造 wasm manifest 缺 `entry` 的 fixture → 脚本 exit=1，预先放置的哨兵 `registry.json`（内容 SENTINEL-REGISTRY）与旧 `.gplugin` 逐字未变，staging/临时文件无残留。

**端到端（真实 guest 构建，非 fixture 字节）**
- 命令（fixture 干跑，不触 web/public）：
  `powershell -File tools\build-plugins.ps1 -ManifestsRoot <tmp-fixture-v2> -OutputDir <tmp>\b2fx-out -RegistryFile <tmp>\b2fx-out\registry.json -ChecksumsFile <tmp>\b2fx-out\sha256sums.txt` → exit=0；keymap/yaml guest 真实 wasm32 构建 + componentize 通过（本机已装 `wasm32-unknown-unknown` target，未触发降 -j）。
- 正式跑（B1 的 v2 manifest 已落树，默认路径）：exit=0；`web/public/plugins/` 产出三个新包，registry v2 原子替换，旧 `gamer.keymap-1.0.0.gplugin`/`gamer.yaml-3.1.0.gplugin` 自动清理。
- 独立复核（Python + sha256sum）：registry 3 条目 id/version/kind 正确、无 signature 字段；每个 `download_url` 文件存在且 sha256/size 与条目一致；`sha256sum -c sha256sums.txt` 4 项全 OK（含 `# source_commit: 31b0eff…` 头）。

**给集成者的重新生成命令**（B1 manifest 再变更时）：

```powershell
powershell -ExecutionPolicy Bypass -File tools\build-plugins.ps1
# 干跑（不动 web/public）：
powershell -ExecutionPolicy Bypass -File tools\build-plugins.ps1 -OutputDir $env:TEMP\plugins -RegistryFile $env:TEMP\registry.json
# Release 完整性清单：
powershell -ExecutionPolicy Bypass -File tools\build-plugins.ps1 -ChecksumsFile <发行目录>\sha256sums.txt
```

## 5. 遗留问题 / 集成依赖

1. **存量验证断言待同步（其他 Agent 所有权）**：HEAD 的 `server/src/api/tests/extensions.rs::official_plugin_market_end_to_end_with_committed_artifacts` 仍断言 registry 只有 2 条目并读 `signature.value`；前端 `registry-client.ts` 的 `REGISTRY_SCHEMA_VERSION=1` 与 `installPolicy` 签名门禁同理——registry v2 + 免签名语义要求 Phase 1 服务端/前端改造合并后这些测试同步翻转，属计划内。
2. ~~`tools/plugin-signing/` 目录整体删除被阻塞~~ **已解决**：服务端 verifier 删除落地后引用消失，目录已整体删除。
3. **keymap guest 源仍在 `server/tests/keymap-guest`**（本轮按计划不迁移，Phase 4 收口）；`$GuestRecipes` 是构建配方（源码路径），版本/元数据不在此表。
4. **publisher 兜底**：manifest v2 未声明 `publisher`，registry 的 `gamer.dev` 来自脚本 `-Publisher` 默认值；若要 manifest 单源，B1 后续在 manifest 加 `publisher` 字段即自动生效（解析已支持）。
5. **Plan §4.2「Registry 与插件包作为 GitHub Release 产物」**：本轮交付到 sha256sums.txt + 可上传产物为止（按任务边界），真实 Release 发布动作未执行。
6. signer 未纳入 server workspace，CI 的 clippy 门禁不覆盖它（历史如此）；本轮已单独跑到 `-D warnings` PASS，后续可考虑纳入独立门禁。
