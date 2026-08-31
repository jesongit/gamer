# Release Manifest v1 契约（ARC-002）

> 状态：**冻结**（批次 0 契约）。依据 `docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md` §6.2 / §11.1 与
> `docs/UPDATE_CONTRACT.md` §6 的产物位置；字段变更必须走 §6 变更规则。
> 本文件、`manifest-v1.schema.json` 与 `validate-manifest.mjs` 三者描述同一契约；冲突时以 fixtures + selftest 实际行为为准。

## 1. 文件清单

| 文件 | 说明 |
|---|---|
| `release/contracts/manifest-v1.schema.json` | JSON Schema（draft 2020-12），结构约束的机器可读定义 |
| `release/contracts/validate-manifest.mjs` | 纯 Node 校验器 CLI（`node:crypto`/`node:fs`，零第三方依赖），规则的可执行定义 |
| `release/contracts/fixtures/keys/test-ed25519-public-1.pem` | 测试用 Ed25519 公钥（**fixture/CI 专用，绝不能用于生产签名**，文件内有注释标注） |
| `release/contracts/fixtures/manifest/valid/` | 合法 fixture（2 份 manifest + 分离签名） |
| `release/contracts/fixtures/manifest/invalid/` | 非法 fixture（24 份，每个只含一种失败原因，文件名即原因） |

## 2. 字段语义

顶层只允许 `schema_version / product / release / platforms`（白名单外字段 = 结构错误）。

| 字段 | 类型/约束 | 语义 |
|---|---|---|
| `schema_version` | `const 1` | manifest 契约版本；本套 schema/校验器只接受 1 |
| `product` | `const "gamebot"` | 产品标识，防跨产品误用 |
| `release.version` | SemVer 2.0.0 | 产品版本；权威源 `server/Cargo.toml`，tag 必须 `v<version>`；**stable 通道禁止 prerelease/build metadata**（校验器强制） |
| `release.channel` | `stable \| beta` | 发布通道；stable 轨道不消费 beta manifest（`--expect-channel`） |
| `release.published_at` | RFC 3339 | 发布时间（UTC） |
| `release.minimum_launcher_version` | SemVer | 能消费本 manifest 的最低 launcher 版本，低于它必须拒绝并提示升级 launcher |
| `release.minimum_upgrade_version` | SemVer | 允许直接升级到本版本的最低起点；更旧安装必须先落中间版本 |
| `release.data_schema` | 整数 ≥1 | 本版本要求的数据布局 schema 版本（SQLite `user_version` + 文件布局）；当前仓库基线 v1 |
| `release.rollback_floor` | 整数 ≥1 | 可回滚的最低 data schema；低于它不在自动回滚承诺内 |
| `release.release_notes_url` | `https://` URL | 发布说明；仅展示，信任不来自 URL |
| `platforms` | 对象，白名单键 | v1 仅 `windows-x86_64`，未知平台整体拒绝；至少含一个平台 |
| `platforms.*.app.artifact` | `{name,url,size,sha256}` | 应用组件包（zip）；`size` 0–2 GiB |
| `platforms.*.app.entrypoint` | 安全相对路径 | 应用入口 `gamer-server.exe`，位于 `versions/<version>/` 内 |
| `platforms.*.components[]` | ≤16 项 | 独立运行依赖（adb/ffmpeg…）；`id` 小写 `[a-z][a-z0-9-]*`，`version` 为上游锁定版本（与 `release/dependencies.lock.toml` 一致） |
| `components[].required_files[]` | ≤1024 项/组件 | 逐文件清单 `{path,size,sha256}`，单文件 0–1 GiB；安装/修复后逐文件校验 |
| `platforms.*.resources.scrcpy_server` | 见下 | `version`（协议锁定 3.3.3）、`path`（必须 `assets/` 前缀，随应用放 `versions/<v>/assets/`）、`sha256`、`binding`（`const "application"`，jar 永远随应用版本更换，不得独立更新） |

hash 一律 **64 位小写 hex SHA-256**；`size` 一律 ≥0；平台内所有声明 size 之和 ≤6 GiB。
URL 仅接受 `https://`（镜像可替换 URL，但信任只来自签名、公钥与内容 hash；浏览器不得覆盖 URL）。

## 3. 签名格式（冻结，launcher 实现以 fixtures 为准）

- **Ed25519 detached 签名，覆盖 manifest 文件原始字节**；launcher 必须**先验签、再解析 JSON**（`invalid/malformed-json-but-signed.json` 演示顺序）。
- `key_id` 位于**签名文件头**（不在 manifest 内，因此 manifest 顶层无 `key_id` 字段）：launcher 读签名头拿到 key_id → 在内置可信公钥表（当前 + 下一把）中查找 → 验签。未知 key_id 直接拒绝（`invalid/unknown-key-id.json`）。
- `.sig` 文件恰好两行（UTF-8，`\n`，容忍结尾空行/`\r\n`）：

  ```text
  gamebot-manifest-sig-1 <key_id>
  <base64，解码后恰为 64 字节 Ed25519 签名>
  ```

  `key_id` 字符集 `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`（禁 `/` `\` `..`，防路径穿越）；base64 必须规范形（`[A-Za-z0-9+/]+=*`，长度 4 的倍数）。任何偏离 = `sig-format-invalid`。
- 测试信任库：`fixtures/keys/<key_id>.pem`（SPKI PEM）。`check` 模式可用 `--key <pem>` 显式指定公钥。
- 私钥只存在于受保护的 Release environment（计划 §6.2/§11.1）；本目录只有测试公钥。

## 4. 校验规则（= 计划 §6.2 / §11.1）

校验顺序 fail closed：读原始字节 → 验签 → 解析 JSON → 显式语义规则 → 结构回退校验（内置迷你 JSON Schema 解释器执行 `manifest-v1.schema.json`）。

- **签名**：未签名（缺 `.sig`）、签名文件格式非法、未知 key_id、验签失败（篡改一字节 / 错误 key）全部拒绝。
- **schema/平台**：`schema_version≠1`、未知平台、未知顶层键、缺必需字段拒绝。
- **版本**：SemVer 格式；`release.version < --expect-current-version` 时按**版本降级**拒绝（低版本默认不覆盖高版本）；`--expect-channel` 不匹配时拒绝（stable 不选 beta）。
- **路径安全**（`entrypoint`、`required_files[].path`、`resources.*.path`、artifact `name`）：仅接受规范化相对路径（`/` 分隔）。拒绝：绝对路径（`/` 开头）、盘符（`X:`）、任何其余冒号（NTFS ADS，如 `adb.exe:hidden`）、反斜杠、`.`/`..` 段、空段、段尾 `.`/空格、Windows 保留名（CON/PRN/AUX/NUL/COM0-9/LPT0-9，按首段扩展名前主名判断，`con.nul` 命中）、非法字符。跨条目：安装树命名空间（entrypoint+required_files+resources）内**大小写不敏感碰撞**与**重复条目**拒绝（Windows 大小写不敏感）。符号链接/reparse point 无法在 manifest 文本层判断，由解包器在落地时拒绝（LCH-006）。
- **hash/size**：大写、长度错、负数、超过压缩包/单文件/总量上限全部拒绝。
- **jar 绑定**：`binding ≠ "application"` 或 `path` 不在 `assets/` 下拒绝。

## 5. Fixtures

合法（`fixtures/manifest/valid/`，均 v0.2.0 / stable / `test-ed25519-public-1` 签名）：

| 文件 | 内容 |
|---|---|
| `manifest-valid-basic.json(.sig)` | 最小合法 manifest：app + adb（1 文件）+ scrcpy_server |
| `manifest-valid-full.json(.sig)` | 完整形态：app + adb（exe+两 DLL）+ ffmpeg + scrcpy_server |

非法（`fixtures/manifest/invalid/`，文件名 = 失败原因 = 校验器错误码）：

| 文件 | 拒绝码 |
|---|---|
| `unsigned-manifest` | `unsigned-manifest`（无 .sig） |
| `tampered-manifest-byte` | `signature-invalid`（manifest 原始字节改 1 字节，用原签名） |
| `wrong-key-signature` | `signature-invalid`（头部声称 key-1，实为其他 key 所签） |
| `sig-format-invalid` | `sig-format-invalid` |
| `unknown-key-id` | `unknown-key-id`（不在信任库的 key_id） |
| `malformed-json-but-signed` | `json-parse-failed`（签名合法但字节非 JSON，验证“先验签再解析”） |
| `unknown-schema-version` | `unknown-schema-version`（=2） |
| `unknown-platform` | `unknown-platform`（多出 `linux-x86_64`） |
| `version-not-semver` | `version-not-semver`（`0.2`） |
| `version-downgrade` | `version-downgrade`（0.1.0 < 期望 0.2.0，需带 `--expect-current-version`） |
| `channel-mismatch` | `channel-mismatch`（beta，期望 stable） |
| `jar-binding-mismatch` | `jar-binding-mismatch`（`binding:"standalone"`） |
| `path-absolute` / `path-drive-letter` / `path-dotdot` / `path-ads-colon` / `path-backslash` / `path-reserved-name` | `path-*`（`/abs/adb.exe`；`C:/evil/adb.exe`；`../../../evil.exe`；`adb.exe:hidden`；`adb\evil.exe`；`con.nul`） |
| `path-case-collision` / `path-duplicate-entry` | `path-case-collision`（`adb.exe`+`ADB.EXE`）；`path-duplicate-entry`（`adb.exe`×2） |
| `sha256-uppercase` / `sha256-wrong-length` | `sha256-uppercase`；`sha256-wrong-length`（63 位） |
| `size-negative` / `size-oversized` | `size-negative`（-1）；`size-oversized`（3 GiB > 2 GiB 上限） |

## 6. 用法

```powershell
# 全量自检：合法必须过、非法必须被对应规则拒绝；全对退出码 0
node release/contracts/validate-manifest.mjs selftest

# 单文件校验（.sig 缺省取 <manifest 去掉 .json>.sig；key 缺省按签名头 key_id 查 fixtures/keys/）
node release/contracts/validate-manifest.mjs check <manifest.json> `
  [--sig <file>] [--key <pem>] [--keys-dir <dir>] `
  [--expect-current-version x.y.z] [--expect-channel stable|beta]
```

退出码：0 通过 / 1 校验失败 / 2 用法错误。selftest 对全部 fixture 统一带
`--expect-current-version 0.2.0 --expect-channel stable`（所有 fixture 的 version 均为 0.2.0，唯 `version-downgrade` 为 0.1.0），并对每个合法 fixture 额外做一次“改一字节必须验签失败”的篡改检查。新增 invalid fixture 必须同步在 `validate-manifest.mjs` 的 `INVALID_EXPECTATIONS` 登记期望错误码，否则 selftest 直接失败。

## 7. 变更规则

- manifest **任何字段/约束变更必须 bump `schema_version`**：新增 `manifest-vN.schema.json` + 对应校验器分支 + 新 fixtures，同时更新本文件；**同步通知所有轨道**（launcher/server/web/CI 共用同一 fixtures，契约变更单独提交）。
- 旧 `schema_version` 永久拒绝（不提供格式迁移）；未知平台、未知顶层键同理，不做隐式兼容。
- 签名格式（magic、key_id 位置、base64 编码）属于本契约的一部分，变更同样需要版本化并更新 fixtures。
- key 轮换：launcher 内置“当前 + 下一把”公钥；新 key 先进 manifest 签名头（key_id 变化），旧 key 退役前须完成双 key 过渡（REL-006 演练）。
- 上限值（size/组件数/文件数）调整须同步 `manifest-v1.schema.json` 与 `validate-manifest.mjs` 两处常量，并新增/更新边界 fixture。
