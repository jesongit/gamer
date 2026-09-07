# Phase 8 证据：Package 分发、插件更新与数据生命周期（计划 §11 + Phase 4 遗留 #5）

> 计划：`docs/plans/gamer_plugin_ecosystem_video_finalization_plan.md` §11、§13.1
> 日期：2026-09-07
> 基线：HEAD `fdffc75`（Phase 3 SDK 已入库），工作树含并行任务（extensions/video 的
> Video Project、web/components/video）在途改动，本任务未触碰其文件。
> 性质：§11.1/§11.2 纵向切片（机制 + REST + 前端 + 测试）；全部结论有实测。

## 1. 完成工作与修改文件

| # | 项 | 文件 |
| --- | --- | --- |
| 1 | 媒体引用登记/解除/查询/删除保护闭环 + 归档媒体恢复 | `server/src/media/mod.rs`（服务层）、`server/src/api/packages.rs`（REST 接线） |
| 2 | Package 导出/导入的媒体分发（`media/**` 白名单 + `?include_media=true`） | `server/src/package_archive.rs`、`server/src/api/packages.rs`、`web/src/api.js`、`web/src/composables/usePackageContext.js`、`web/src/workspace/PackageContextBar.vue` |
| 3 | 插件更新语义收口（§11.2） | `server/src/extensions/service.rs`、`server/src/api/extensions_management.rs` |
| 4 | Phase 4 审计遗留 #5：keymap profile 门禁去 id 字符串比较 | `server/src/api/extensions.rs`、`server/src/extensions/mod.rs`（仅 re-export 两行） |
| 5 | 测试 | `server/src/media/mod.rs`（内联 +6）、`server/src/package_archive.rs`（内联 +3）、`server/src/api/tests/packages.rs`（+1 REST 全链）、`server/src/extensions/service.rs`（内联 +5）、`web/src/package-context.test.js`、`web/src/api.test.js` |
| 6 | 本证据 + 坑记录 | `docs/evidence/phase8_package_media_lifecycle.md`、`docs/PITFALLS.md` |

所有权偏离说明：`web/src/api.js` 不在授权清单内，但 `exportPackageArchive` 是
include_media 选项的唯一传输面（组合式函数的注入 seam 只能覆盖测试），改动为
最小签名扩展（第二参数可选对象 + 查询串拼接，12 行）；并行 Agent 所有权文件
（api/media.rs、extensions/video|gamer_yaml|keymap、web/components/video|console、tools）零改动。

## 2. §11.1 媒体引用生命周期（契约与实现）

### 2.1 写入方对接说明（给视频项目等业务写入方）

引用本体 = `MediaRef { package_id, plugin_id, kind }`（`kind` 如 `project`，
Core 不解释）。**引用存放在媒体库的 metadata.json（media 侧）**，不在包内；
包内项目文件按 media id（+ sha256 内容身份）引用素材。

- **登记**（项目保存/素材关联时）：
  `POST /api/media/:id/refs`，body `{"refs":[{package_id, plugin_id, kind}]}` ——
  **全量替换**语义（以项目当前引用集合为准的声明式同步；幂等可重放）。
  服务层增量形态：`MediaService::add_ref(id, ref)` / `remove_ref(id, &ref)`
  （幂等单条，服务端内部钩子与扩展边界用）。
- **解除**：项目删除素材引用时同样走全量替换（去掉对应条目）；或服务层
  `remove_ref`。
- **查询**：`GET /api/media/:id`（metadata.refs）与
  `GET /api/packages/:pkg`（响应新增 `media_refs: [{id,name,sha256,size,
  plugin_id,kind,state}]` + `media_total_bytes`（按 sha 去重））。
- **删除保护**：refs 非空 → `DELETE /api/media/:id` 409 `media_referenced`。
- **GC 选型（记录）**：**显式解除，无后台扫描**。两个服务端钩子保证引用不
  悬挂：① `DELETE /api/packages/:pkg` → `MediaService::release_package(pkg)`
  自动解除该包全部引用；② `POST /api/packages/:pkg/duplicate` → 为副本登记
  同一素材的引用。引用清空后素材经既有删除端点回收。
- **覆盖导入**：包数据整体替换语义 → 先 `release_package` 再按新归档索引
  重新登记（旧引用不残留）。

视频项目（并行 Agent 的 `plugins/gamer.video/projects/<id>.json`）对接建议：
保存项目时收集项目内引用的 media id 集合，经全量替换端点同步 refs
（`plugin_id="gamer.video"`, `kind="project"`）——删除保护与导出引用登记即自动生效。

### 2.2 归档媒体分发（**布局白名单破坏性变更**）

- 归档布局白名单从 `package.toml` + `shared/**` + `plugins/<id>/**` 扩展为
  **另加受控目录 `media/`**：仅 `media/index.json` 与 `media/files/<64hex>`
  两种形态（files 段必须是 64 位 hex 内容哈希；其余 `media/**` 拒绝）。
  **破坏性点：旧版本服务端会拒绝含 `media/` 的新归档；旧归档（无 media/）
  在新服务端照常导入**（白名单只增不改）。
- `media/index.json`（schema v1，≤1MiB/≤1024 条目）：
  `{schema_version, entries:[{id, name, sha256, size, plugin_id, kind, included,
  probe?{container,codec,width,height,rotation,duration_us}}]}`。逻辑 id =
  导出侧 MediaId。`included` 必须与 `media/files/<sha>` 存在性一致，否则整体拒绝。
- **默认导出不含原始大视频**：只写 `media/index.json`（引用登记/缺失标注，
  `included:false`）；`POST /api/packages/:pkg/export?include_media=true` 追加
  素材字节。单素材上限 90 MiB（媒体专用上限，不走插件资源 10 MiB），归档
  总预算 100 MiB 不变。导出可复现性保持（索引 + 素材一并按路径排序、固定 mtime）。
- **导入（含素材）**：解压 staging 内校验路径/大小/sha256/逻辑 id → 逐条
  `MediaService::restore_media`（暂存目录落盘 → 原子改名提交；元数据随索引
  恢复**不重跑 ffprobe**）→ 任一条失败回滚本次新建素材并整体拒绝（无半成品）。
  响应新增 `media: {total, imported, reused, reattached, missing}` 摘要。
- **逻辑 id 保留策略**：库中无该 id → 原样新建（包内按 id 的项目引用导入即
  有效）；同 id 同 sha → 复用仅补引用（Reused）；同 id 不同 sha → 换新 id
  导入（Collided，旧 id 引用需用户重新关联——记录为已知边界）。
- **不含素材导入**：`included:false` 条目按 id+sha 自动重挂库内已有素材
  （reattach）；匹配不到 → 保留缺失状态（索引已随包落盘，可再导入含素材包
  或手动登记引用恢复，无需重做项目）。

### 2.3 前端

- 导出按钮新流程：先查 `GET /api/packages/:pkg` 的 `media_refs` → 无素材直接
  导出（行为同旧）；有素材弹确认框：素材清单（名称/大小/插件/用途）+ 合计
  大小 + 隐私提示（「素材为原始录屏画面，可能包含敏感信息」）+ 「包含媒体
  素材」勾选（默认关）。`api.exportPackageArchive(id, { includeMedia })` →
  `?include_media=true`。

## 3. §11.2 插件更新语义变化清单

| 语义 | 现状/变化 | 落点 |
| --- | --- | --- |
| 不可变版本 + active_version + 失败更新不破坏 | 核验保留：update 先 inspect 后 install_archive（目录已存在 → AlreadyInstalled，记录/指针不动）；activate_version 回滚不复制不删除。新增测试锁定 | `service.rs` 测试 ×2 |
| 官方按 id/SemVer/host_api 检查更新 | 核验：服务端无 URL 拉取路径（更新恒为归档上传 + 可选 `x-expected-sha256` 完整性钉 + 权限增量确认）；registry 版本对比在前端 registry-client（sha256 校验保留）。本地插件无从不明来源自动更新的服务端路径（不存在即不发生） | 无需改动 |
| wasm 包冒充 builtin id（usurp） | **新增拒绝**：`execution.kind=wasm` 且 id 在宿主 builtin 注册表 → `InvalidManifest`（全新安装与 builtin→wasm 降级都拒）；错误经 `extension_error` → 400 | `service.rs::execution_policy_check` |
| builtin 别名包 | **新增拒绝**：`kind=builtin` 的包 id 必须 = `builtin_id`（实现即扩展） | 同上 |
| wasm→builtin 官方迁移 | 放行（仅对已装 wasm legacy 版本的 update），inspect 响应新增 **`execution_change: {from, to}`**，确认弹窗必须提示；builtin→wasm 恒拒 | `service.rs` + `extensions_management.rs` |
| 未知 builtin_id | 已有：`HostFeatureUnavailable`（409，提示升级宿主） | `inspect_compatible`（不动） |
| 权限增量确认 | 已有（install/update 共用 `ensure_permission_confirmation`） | 不动 |
| 卸载插件 ≠ 删除 Package 数据 | 核验锁定：uninstall 只删 `extensions/<id>/<version>/`（+ 可选 `?delete_data` 删 `extension-data/<id>`）；`packages/<pkg>/plugins/**` 与媒体引用不受影响（dormant 测试 + REST 卸载测试双锁定） | 无需改动 |
| keymap profile 门禁去 id 比较（Phase 4 遗留 #5） | api 层改用 keymap 边界谓词 `is_keymap_extension`（经 extensions/mod.rs re-export，api 不再持有 `KEYMAP_EXTENSION_ID` 字面量比较）；谓词本体在 keymap/mod.rs 只读复用 | `api/extensions.rs` |

## 4. 测试结果（本机，`CARGO_PROFILE_DEV_DEBUG=0`，`-j 4`）

| 命令 | 结果 |
| --- | --- |
| `cargo check -j 4`（全部编辑后，含并行在途改动合树） | PASS |
| `cargo clippy -j 4 --all-targets -- -D warnings` | PASS（0 警告） |
| `cargo fmt --all -- --check` | PASS（fmt 后） |
| `cargo test -- media::tests package_archive::tests packages_tests extensions::service::tests extensions::builtin` | **52 passed / 0 failed**（含新增 media 引用闭环 4 项、归档媒体 3 项、REST 媒体导入全链 1 项、更新语义 5 项） |
| `cargo test -- packages_dormant_tests packages_states_tests extension_rest_lifecycle core_runtime_contributions declarative_plugin_call_roundtrip official_plugin_market` | **9 passed / 0 failed** |
| `cargo test -- architecture_guard` | **7 passed / 0 failed** |
| `pnpm test:run`（web 全量，含并行新增 video-project 测试） | **62 文件 787 passed / 0 failed** |

未验证：`cargo test` 全量（本机内存墙，按 PITFALLS 惯例以过滤组替代）；
真实双机 .gamerpkg 媒体分发人工链路（机制由 REST 全链测试覆盖）。

## 5. 遗留问题

1. **同 id 不同内容碰撞**（`Collided`）：换新 id 导入后，包内按旧 id 的项目
   引用需用户重新关联；「手动选已有媒体重关联」的 UI 未做（sha 自动匹配已覆盖
   常规场景，计划验收项「重新关联」以再导入含素材包为最小路径）。
2. **AGENTS.md / 归档布局文档同步**：布局白名单已含 `media/**`，AGENTS.md 的
   .gamerpkg 描述与 docs/yaml-v3 之外的参考文档归 Phase 9 §12.3 文档收口。
3. **前端 plugin-center 对 `execution_change` 的确认弹窗展示**未接线（服务端
   inspect 已透传；plugin-center 属并行/后续轮次文件）。
4. **`media_total_bytes`/`media_refs` 走 `GET /api/packages/:pkg` 详情响应**
   （未加独立端点：api/mod.rs 非本任务所有权，路由一次定型原则）；若详情
   变重可后续拆 `GET .../media-refs`。
5. Phase 4 审计遗留 #4（`extensions/mod.rs::native_call_action` gamer.yaml
   分发缝注册表化）不在本轮范围，维持「后续做」。
