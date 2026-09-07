# Phase 1 前端证据：免签名安装体验 + 市场展示（B3）

> 计划：`docs/plans/gamer_plugin_ecosystem_video_finalization_plan.md` §4.1（安装体验）+ §4.3（市场展示）
> 日期：2026-09-07
> 基线：`31b0eff`（Phase 0 基线 HEAD）；本报告只覆盖前端（web/），服务端与构建脚本由并行 Agent 改造（工作树中已有对应改动）。
> 性质：全部结论基于本工作树实际运行 `pnpm test:run` 与 `pnpm build` 的结果，未把旧报告当作通过依据。

## 1. 完成项

| 计划条目 | 状态 | 说明 |
| --- | --- | --- |
| §4.1 移除前端签名/proof 门禁 | 完成 | `installPolicy` 不再读取签名状态，所有来源 `allowed: true`；`registryProofFor` 整体删除；official 安装不再上传 `X-Gamer-Registry-Proof` 头；市场按钮不再因「签名未验证」阻断 |
| §4.1 官方下载强制 SHA-256 | 保留 | `downloadFixedVersion` 哈希/大小校验原样保留；官方条目缺 sha256 拒绝下载；哈希不匹配文案明确「文件与索引不一致……不污染已装版本」 |
| §4.1 安装确认弹窗改版 | 完成 | 展示 ID、版本、来源（官方市场/本地导入/URL 导入）、执行形态（WASM 插件 / 宿主预置（需要 Gamer 宿主支持））、宿主版本要求（host_version 缺失不显示）、完整权限清单 + 相对已装版本的权限增量；界面不再出现密钥/签名/proof 字样 |
| §4.3 市场列表 | 完成 | PluginCenter 市场 tab 渲染 registry v2 三插件（gamer.yaml@3.1.1 / gamer.keymap@1.0.1 / gamer.video@1.0.0）；卡片展示版本、执行形态（builtin 标注宿主预置 + 警示色）、已安装状态、宿主要求、权限、UI 类型；读端容忍 v1（无 execution 视为 wasm、有 signature 字段忽略不报错） |
| §4.3 错误提示分型 | 完成 | `installErrorText`：下载网络错误/超时、404、HTTP 错误、哈希不匹配、缺 sha256、registry 无效/不受支持、`host_feature_unavailable`（→「升级 Gamer」，明示重试无意义）、权限未确认、已安装；registry-client 自身 RegistryError 文案已人话化直接透传 |
| §4.3 安装后无需刷新服务端 | 确认 | 前端链路：inspect → confirm → install（服务端即装即启）→ 需要时补 enable → `emit('changed')` + `refresh()` 重拉 management 清单；组件测试断言 `enableExtension` 被调、提示可见（顺带修正安装提示被 refresh 清掉的时序，同 activateVersion 既有做法） |
| §4.1 本地/URL 导入无签名可用 | 完成 | 本地导入文案改为「不要求签名」，URL 导入仅提示「来源不属于官方市场」；两者仅产生确认提醒，不阻断 |
| ui_contributions 仅 Running 下发（契约 5） | 确认无需改 | 前端消费链（`adapter/server-ui.ts` → `contribution-manager.ts::registerServerUiContributions`）本来就是「服务端给什么显示什么」，无按 state 补挂逻辑 |

## 2. 修改文件（全部在授权范围内）

| 文件 | 改动 |
| --- | --- |
| `web/src/workspace/plugin-center/registry-client.ts` | `REGISTRY_SCHEMA_VERSION=2`；schema_version 接受 {1,2}（v1 容忍）；删除 `normaliseSignature`（signature 字段存在则忽略）；新增 `normaliseExecution`（缺省/未知值归一 wasm，builtin 需显式声明，透传 host_version）；`downloadFixedVersion`：404 独立错误码 `download_not_found`、网络错误文案带「超时」、hash_mismatch 文案改写 |
| `web/src/workspace/plugin-center/types.ts` | 删除 `PluginSignature`/`SignatureStatus` 及各接口 signature 字段；registry 条目 `execution` 变为必填 `PluginExecution{kind:'wasm'\|'builtin', host_version?}`；`InstalledPluginSnapshot`/`PluginInspection`/`PluginInstallSource` 增加可选 `execution` |
| `web/src/workspace/plugin-center/plugin-service.ts` | 删除 `normalizeSignature`/`registryProofFor`/`signatureLabel`；来源元数据（localStorage）不再存签名；`mergeManagementResponse` 不再回填签名、改为透传 `execution`（inspect 优先、registry 兜底）；`installPolicy(source)` 单参、恒 allowed、非官方来源仅 warning；新增 `normalizeExecution`/`executionLabel`/`hostVersionLabel`/`installErrorText` |
| `web/src/workspace/plugin-center/PluginCenter.vue` | `canInstallMarket = !!entry.sha256`（官方固定 hash 门禁，hint 文案更新）；`installMarket` 阻断文案与错误路径改用 `installErrorText`；`installArchive` official 只传 `{source:'official'}`（无 proof）；`inspectAndConfirm` 弹窗按 §1 改版；市场卡/已装卡删除签名 tag、新增执行形态 tag（builtin 警示色）、已安装版本 tag、宿主要求行；本地/URL 导入文案去签名语义；安装成功 notice 移到 refresh 之后 |
| `web/src/workspace/MarketView.vue` | 已装插件清单每项增加执行形态徽标（服务端 snapshot 带 `execution.kind==='builtin'` 时显示「宿主预置[ · host_version]」，缺失按 WASM 显示，不阻塞列表） |
| `web/src/plugin-center.test.js` | 语义翻转 + 新增：v2 registry 归一（三插件含 builtin video）、v1 容忍（无 execution→wasm、signature 忽略）、哈希不匹配拒绝（码 + 文案）、缺 sha256 拒下载、installPolicy 全来源放行、installErrorText 分型、official 安装不再发 proof 头（source/permission 头保留）、execution 保守归一 |
| `web/src/plugin-center-activate.test.js` | 追加「Phase 1 免签名市场」describe（happy-dom 挂载 PluginCenter）：v2 registry 三插件渲染（video builtin + 宿主要求 + 界面无「签名」字样 + 安装按钮可用）、确认弹窗内容断言（ID/版本/来源/执行形态/宿主要求/权限增量/无 proof 概念/inspect+install 选项断言/enable 补齐）、`host_feature_unavailable` →「升级 Gamer」；既有版本切换测试未动（其 fixture 中 `signature: {status:'valid'}` 字段被前端忽略，仍通过） |

未改动 `api.js`（不在所有权内也不需要：`extensionUploadOptions` 收不到 `registryProof` 时自然不发该头；`X-Gamer-Extension-Source: official` 保留用于服务端来源记录）。未改 `console-shell-smoke.test.js`（其 registry stub 为 `{schema_version:1, plugins:[]}`，v1 容忍下原样通过）。

## 3. 契约消费点（前端视角）

1. **registry v2**：`schema_version ∈ {1,2}`；条目 `execution{kind,host_version?}`；`signature` 字段忽略。产出侧应为 v2（`REGISTRY_SCHEMA_VERSION=2` 仅表达当前产出基线）。
2. **inspect/snapshot**：`execution{kind,host_version?}` 可选读取（inspect 优先于 registry 条目）；`signature` 字段预期被移除，前端已无任何读取点（若旧服务端仍下发也被 `{...item}` 透传但无 UI 消费）。
3. **安装请求头**：保留 `X-Gamer-Extension-Source: official`（来源记录）与 `X-Gamer-Permission-Confirm: 1`；不再发送 `X-Gamer-Registry-Proof`。
4. **错误码**：`host_feature_unavailable` → 升级提示；`permission_confirmation_required` → 权限确认提示；其余服务端错误透传 `message`。
5. **完整性**：官方下载强制 `sha256` + 20MiB 上限，不匹配错误码 `hash_mismatch`。

## 4. 测试结果

命令（web/ 下）：

```
pnpm test:run        # vitest run（全量 61 文件）
pnpm build           # 构建校验（vue-tsc/vite 产物，输出 ../server/web-dist/）
```

结果：

- `pnpm test:run`：**Test Files 61 passed (61)，Tests 748 passed (748)**（2026-09-07 实测，含本次翻转/新增的 `plugin-center.test.js` 11 项与 `plugin-center-activate.test.js` 追加 3 项）
- `pnpm build`：**built in 2.12s**，无类型/编译错误（仅有既有的大 chunk 警告）

## 5. 对后端契约的假设清单（供集成者核对）

| # | 假设（字段名精确） | 若不符的影响面 |
| --- | --- | --- |
| 1 | inspect 响应与 management snapshot 的条目可带 `execution: { kind: "wasm"\|"builtin", host_version?: string }`；`kind` 缺失/未知时前端按 wasm 展示（不阻塞） | 仅展示面；缺失时 video 会被标成「WASM 插件」（误导），建议 snapshot 至少透传 `execution.kind` |
| 2 | `GET /api/extensions`（无 management 回退路径）同样不要求 signature 字段；snapshot 的 `signature` 字段被移除或忽略均可 | 无（前端零读取） |
| 3 | 安装/更新请求不再需要 `X-Gamer-Registry-Proof` 头；`X-Gamer-Extension-Source: official` 头仍被解析（来源记录用）；`X-Gamer-Permission-Confirm: 1` 仍是权限增量确认的唯一凭证 | 若 official 来源头被废弃，仅影响来源标注，不影响安装成功 |
| 4 | builtin 安装到不支持宿主时返回结构化错误码 `code: "host_feature_unavailable"`（`ApiError::with_code`，前端经 `error.code` 读取，HTTP 状态不限） | 若码名不同，前端退化为透传服务端 message，「升级 Gamer」提示不会出现 |
| 5 | 权限增量未确认时返回 `code: "permission_confirmation_required"`（现服务端 `PermissionConfirmationRequired` → 409，code 字段目前未附） | 前端仍可工作（透传 message），仅失去定制文案 |
| 6 | registry.json（B2 产出）：`schema_version: 2`、三插件条目均带 `sha256`（64hex，官方条目缺 sha256 会被前端拒下载）、`download_url` 为 http(s) 或同源绝对路径、`execution.kind` 缺省视为 wasm | `schema_version=3` 会被前端拒绝（`unsupported_registry`），需同步升 `SUPPORTED_REGISTRY_SCHEMA_VERSIONS` |
| 7 | `POST /api/extensions` 即装即启（安装响应 state='installed' 时前端补调 enable），无需服务端重启 | 现行为已满足；仅提示链路依赖 |
| 8 | 版本号：gamer.yaml→3.1.1、gamer.keymap→1.0.1、gamer.video→1.0.0（测试 fixture 按此写死，仅作数据不校验产物） | 无硬依赖 |

## 6. 遗留问题 / 交接收口

- 集成时需用 B2 产出的真实 `web/public/registry.json`（v2、含 video）替换后做一次手动市场冒烟（本地 dev 未起，按任务约束未验证浏览器实机链路）。
- `plugin-center-activate.test.js` 既有 fixture 仍带 `signature: {status:'valid'}` 字段（第 26 行）——前端已忽略该字段，测试通过；后续清理该 fixture 可选。
- `installPolicy` 现恒 `allowed:true`，函数保留是为承载「非官方来源确认提醒」语义；若 Phase 3 需要来源细分（如 url 黑名单）在此扩展。
- `uiType(entry)` 的 'declarative' 文案对 `runtime="core"` 贡献不够精确（既有行为，未在本轮扩大范围）。
