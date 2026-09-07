# Phase 9 最终验收报告：统一验收、文档与清理

> 计划：`docs/plans/gamer_plugin_ecosystem_video_finalization_plan.md` §12（Phase 9）
> 日期：2026-09-08 · 分支 main · HEAD `db4d481`（feat(engine): 模板制作、离线测试与 YAML 草稿闭环（Phase 7））
> 工作树改动（本轮，未提交，由集成者收尾）：AGENTS.md、README.md、ADR-15（新增）、两个计划状态回写、
> `docs/evidence/phase4_ownership_audit.md` §5 复核、`web/src/api.js` 死代码清理、
> `web/public/plugins/*.gplugin` + `registry.json`（build-tools 重跑产物，sha256 轮换、自洽）。
> `server/data/` 为运行数据不入库。

## 1. 测试命令与退出码汇总（本机 Windows，Git Bash）

内存墙规避：`CARGO_PROFILE_DEV_DEBUG=0` + `-j 4`（PITFALLS 2026-09-07；未 cargo clean，增量缓存完好）。

| 命令 | 结果 | 退出码 |
| --- | --- | --- |
| `cd server && CARGO_PROFILE_DEV_DEBUG=0 cargo test -j 4 -- --test-threads=2`（全量，含 architecture_guard 七测试） | **658 passed / 0 failed** / 6 ignored（98.99s） | 0 |
| `cd web && pnpm test:run`（vitest 全量，含 core-shell-boundary） | **64 files / 810 passed / 0 failed** | 0 |
| `bash tools/e2e_phase7_offline.sh`（无设备 E2E，临时端口 18443 + 临时数据目录） | **E2E ALL GREEN**（登录→ffmpeg 造视频→导入→帧身份→建包→免签装启 gamer.yaml→动作缝建模板（8-bit 灰度实测）→vision/test 指定帧命中（score 1.0，帧身份一致）→create_draft/save_draft→automations 列表→重名拒绝→yaml stop 后动作 409、vision 不受影响） | 0 |
| `powershell -File tools/ci-local.ps1`（env：CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS=4；等效 ci.yml 工作流） | **全部 10 门禁 PASS**（见 §2） | 0 |
| `powershell -File tools/build-plugins.ps1`（重跑） | 三包 + registry v2 重建，staging 自检全过 | 0 |
| `cargo test -- official_plugin_market`（产物重跑后复验在库产物哈希/安装链） | 1 passed / 0 failed | 0 |
| `npx vitest run src/api.test.js`（api.js 清理后定点复验） | 14 passed / 0 failed | 0 |

### 2. ci-local.ps1 各 gate 明细（exit=0）

| Gate | 结果 | 耗时 |
| --- | --- | --- |
| cargo fmt --check | PASS | 1s |
| cargo clippy --all-targets --all-features -- -D warnings | PASS | 35.7s |
| cargo check --locked --no-default-features（无 WASM 退出路径防退化） | PASS | 26.5s |
| guest: build gamer-yaml-guest (wasm32) + Component 校验 | PASS | 0.6s |
| cargo test | PASS | 20.6s（增量） |
| cargo build --release | PASS | 148.3s |
| pnpm install --frozen-lockfile / test:run / build | PASS | 2.5s / 9.1s / 4.2s |

## 3. 测试矩阵（计划 §12.1，15 行逐项真实状态）

| # | 场景 | 结果 | 证据（测试名/命令/提交） |
| --- | --- | --- | --- |
| 1 | 免签名：真实 WASM、官方无签名包、无私钥构建 | **PASS** | `official_plugin_market_end_to_end_with_committed_artifacts`（官方包免 proof + `x-expected-sha256` 安装即 Running）；Phase 3 SDK 示例隔离实例 install→call→stop→uninstall 实测（phase3_sdk_examples/fix_runtime_imports）；signer 零密钥 pack/verify（3dac760） |
| 2 | 包完整性：哈希错/缺文件/穿越/重复/膨胀/坏 manifest | **PASS** | archive.rs 中央目录/穿越/重复/加密/限额测试；`x-expected-sha256` 不符 400；前端 hash_mismatch 拒装（plugin-center.test.js）；signer verify 反例套件（B2 §4） |
| 3 | 宿主边界：伪装 builtin/未知 builtin id/伪造 core 组件/任意入口 | **PASS** | builtin 包禁携带 plugin.wasm（防伪装测试）；未知 builtin_id → 409 `host_feature_unavailable`；wasm 冒充 builtin id/builtin 别名/builtin→wasm 拒绝（service.rs execution_policy_check 测试，58f5f27）；declarative call 白名单（CallRejected→400）；core 组件名由前端注册表白名单解释（core-component-registry.test.js） |
| 4 | 权限：新增权限/拒绝授权/跨 Package/跨插件私有资源/伪造 caller | **PASS** | 权限闭集 19 项 + 增量确认头测试；vision-probe 未授权 device.read 实测 denied（phase3 §3.1）；插件资源隔离（不能读写他插件目录/shared/）测试；dormant 数据回归 |
| 5 | UI 隔离：恶意 iframe/未知动作/错误来源/冒充宿主 | **PASS（逻辑层）**；浏览器攻击面人工点检 NOT_VERIFIED | iframe-plugin-call.test.js（origin/动作校验）、declarative-panel-host.test.js、ui.rs schema 校验；UI 贡献仅 Running（guard 全链测试） |
| 6 | 官方市场：三插件列表/下载/安装/更新/404/网络失败/哈希错误 | **PASS（逻辑+REST）**；真实 GitHub Release 链路 NOT_VERIFIED | plugin-center.test.js / plugin-center-activate.test.js（v2 渲染、哈希拒绝、404/超时/host_feature_unavailable 分型、无签名可装）；官方市场 REST 端到端（在库产物） |
| 7 | 用户插件：仓库外创建→构建→导入→启动→调用→存数据→更新→卸载 | **PASS** | sdk/examples 三示例独立 crate 全链实测（echo-minimal 生命周期、hello call/Package 私有数据/app_context、vision-probe 权限拒绝）；更新语义（失败更新不破坏、activate_version 回归、execution_change 提示）测试 |
| 8 | 视频无设备：导入/播放/逐帧/项目/标记/校准/离线匹配，不触 ADB | **PASS** | `tools/e2e_phase7_offline.sh`（adb_path 故意缺失）全绿；media/recording 单测；vision/test media_id+pts_us/frame_index 离线寻址与 DeviceManager 类型层隔离；Video Project 无设备 E2E（d44a959）。UI 人工走查 NOT_VERIFIED |
| 9 | 视频输入：媒体模式鼠标/键盘/Keymap/舞台输入、切回实时 | **PASS（逻辑层）**；实机点检 NOT_VERIFIED | console-stage.test.js（23 项：guardDeviceInput、媒体模式路由拒绝、generation 过期）；切回实时不恢复旧按键状态由 StageSource 状态机测试覆盖 |
| 10 | 录制：后台录制/断连/编码变化/磁盘不足/重复停止/异常退出 | **PASS（机制单测）**；真机后台录制 NOT_VERIFIED | recording 分段状态机/设备独占/stop·cancel 幂等/`on_device_session_boundary` 单测；等 IDR/背压丢帧计数单测 |
| 11 | 输入事件：来源标注、去重、敏感文本 | **PASS（单测+修复）**；真机事件核对 NOT_VERIFIED | 92cf82c 来源标注（manual/keymap/runner/plugin task-local scope）+ 回归测试；operation_id 去重、text 脱敏单测 |
| 12 | 帧与坐标：多 fps/VFR/旋转/竖屏/黑边/参考尺寸 | **PASS（机制）**；样本矩阵实测 NOT_VERIFIED | 7b85745（libx264 -bf 2 + VFR 样本逐帧一致性、B 帧解码序→展示序归一、frames.json 上界/重建/并发去重）；calibration.js 四级坐标变换测试（非等比拉伸结构性排除） |
| 13 | 模板/YAML：定帧裁切/离线匹配/草稿生成保存编辑/缺依赖提示 | **PASS** | e2e 全链 + template-studio.test.js / yaml-capability.test.js（yaml 未 Running 制作入口禁用+提示）；草稿不自动执行（不建任务不启 Runner）；真机显式运行 NOT_VERIFIED |
| 14 | Package：不含/含媒体、缺失引用、覆盖/复制、卸载重装 | **PASS（REST 全链）**；真实双机人工链路 NOT_VERIFIED | package_archive media/** 三态导入测试、REST 媒体导入全链、引用闭环 4 项、?include_media 导出、dormant/覆盖/复制测试（58f5f27） |
| 15 | 平台：Windows 完整包/Docker/直跑/真机模拟器 | **部分** | Windows 开发机（本报告全部证据 + release build PASS）；Docker/直跑/真机矩阵 NOT_VERIFIED；CI ubuntu 有等效工作流（本轮未触发，本地已复现全部门禁） |

## 4. 平台与构建证据

- 构建门禁：§2 十门禁全绿（含 `cargo build --release` 148.3s，release 产物可直接运行——无需降级 -j 2）。
- 官方插件产物（build-plugins.ps1 重跑，exit=0）：`gamer.keymap-1.0.1.gplugin`(wasm)、`gamer.video-1.0.0.gplugin`(builtin)、`gamer.yaml-3.1.1.gplugin`(wasm)；registry `schema_version=2`、无 signature 字段、三条目 sha256/size 与文件逐字对账通过（Python 独立复算）；产物重跑后 `official_plugin_market_end_to_end_with_committed_artifacts` 复验 PASS。
- wasm32 target 已安装（无环境缺口）；guest wasm32 构建 + Component 校验双关卡绿。
- sha256sums.txt 为 `-ChecksumsFile` 可选产物（默认构建不生成；发布时显式传参，B2 §3）。

## 5. NOT_VERIFIED 清单（含执行建议）

| 项 | 原因 | 建议 |
| --- | --- | --- |
| 真机 adb 全链路：录制→草稿→真机显式运行、start_app/投屏联动、多 viewer、看门狗交互 | 无设备环境；计划 §13.1-5 禁止无证据推断 | **由集成者在真机环境另行执行**（一次性操作按计划用模拟目标/测试账号） |
| ~~浏览器实机点检：市场安装全链手动冒烟、视频工作台 UI 人工走查~~ | — | **已完成**（集成者 2026-09-07：隔离 18443/5174 实测三插件市场/免签安装/面板出现/UI 正常，发现并修复业务面板激活回归与 uiType 标签，813 项 vitest 全绿，见 phase9_browser_smoke.md）；多页面互斥实机走查仍待真机环境 |
| 真实 GitHub Release 发布与下载（registry download_url 远端拉取） | 本轮仅交付本地产物 + 可上传清单 | 发布时执行 `build-plugins.ps1 -ChecksumsFile` 并走一次真实安装 |
| 性能基线（§12.2：录制开关开销/精确帧延迟/并发解码/CPU/内存/磁盘/长录） | 未建立量化数据（只有机制正确性测试） | 后续独立轮次，不凭空设预算 |
| Docker / 直跑平台矩阵、FFmpeg 路径发现兼容矩阵 | 本机仅 Windows 开发环境 | CI/部署侧补充 |
| H.264 兼容样本矩阵（720p~4K/竖屏/黑边/旋转实测样本；VFR/B 帧已有机制验证） | 样本集未准备 | 后续补 |
| `cargo test`（Linux/CI 平台）与 `cargo test --release` 本机复跑 | 本机 Windows；CI 有等效工作流 | CI 触发即为证据 |
| 多指/旋转/黑边素材上的模板制作人工体验 | 机制有坐标/校准测试覆盖，体验未人工验证 | 随真机点检一并 |

## 6. 剩余问题清单（各 evidence 遗留节汇总去重）

**P1（交付判定依赖，需集成者执行/裁决）**

1. 真机全链路（录制→草稿→真机显式运行、多页面互斥）——浏览器实机点检已完成（phase9_browser_smoke.md，冒烟中发现并修复业务面板激活回归），真机项是剩余唯一实机证据缺口。
2. 性能基线未建立（§12.2 明确要求"真实基线、不凭空设定"）——不阻塞功能交付，阻塞"性能可交付"结论。

**P2（记录在案，不阻塞）**

1. keymap `android_keycode` 双份词表漂移（host `keymap/mod.rs` / guest `guests/keymap-guest/src/lib.rs`），待单一来源（phase4 复核 #3）。
2. `guests/keymap-guest/ui/index.html` 遗留 iframe 资产：官方包不携带，仅测试用；可删并改内联 HTML（phase4 #4）。
3. 前端业务面板（自动化/函数/模板/映射/视频）仍为 runtime=core 宿主组件随宿主前端发布，独立 UI 资产化留后续（phase4 #5）。
4. 媒体导入 `Collided`（同 id 不同 sha）后缺「手动重关联已有素材」UI；最小路径 = 再导入含素材包（phase8 #1）。
5. `media_refs`/`media_total_bytes` 走 `GET /api/packages/:pkg` 详情响应，未拆独立端点（phase8 #4）。
6. builtin 伪装检测按文件名（只禁 `plugin.wasm`，包内其他 `*.wasm` 不拒），需要时可收紧（B1 偏差 2）。
7. registry `publisher` 来自脚本 `-Publisher` 兜底（manifest 未声明 publisher 字段；signer 解析已支持，加字段即单源）（B2 #4）。
8. signer 未纳入 server workspace，CI clippy 门禁不覆盖（本轮已单独 `-D warnings` PASS）（B2 #6）。
9. `plugin-center-activate.test.js` 旧 fixture 仍带 `signature:{status:'valid'}` 字段（前端已忽略，可选清理）（B3）。
10. `build_guest_fixture_component`/`package_guest_fixture_gplugin` 函数名带 "fixture"（语义已注释澄清，改名需动 api/tests）（phase4 #6）。

## 7. 本轮清理与文档改动

- **死引用清理**：`web/src/api.js` 删除 `registryProof` 请求头分支与 `base64Utf8` 助手（免签名收口后零调用方，PITFALLS 假绿形态排除——全仓 grep 确认无 caller）；同步修正 api.js 内指向 Registry proof/包签名的过时注释。保留：`tools/plugin-signer` 的 keygen/sign（存量签名包应急路径，默认链不调用）；launcher/主程序更新签名全链（verify-release.ps1、release/packaging、`launcher/src/manifest/sig.rs`）明确不在清理范围。
- **文档**：AGENTS.md（免签名/manifest v2/builtin 注册表/官方版本/keymap guest 转正/UI 仅 Running/registry v2/媒体帧表/Video Project/media 白名单/引用契约/actions.rs/sdk/来源标注/x-expected-sha256，删签名与 keypair 描述）；README.md（插件/市场/视频特性、目录结构、数据目录、API 表删除已退役的 `/api/apps/:app/resources`、`/api/app-packages/*`、`/api/workspace` 行，launcher 签名描述保留）；`docs/reference/adr/ADR-15-plugin-unsigned-install-and-execution-kind.md`（新增）；两个计划状态回写；phase4 审计「后续做」复核（#1/#2 已闭环、#3-#6 确认仍有效）。
- **验证**：`web/public/registry.json` 无 signature 字段、schema v2、三包 sha256/size 逐字对账一致。
