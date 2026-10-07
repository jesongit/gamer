# 多画面与独立运行实施验收记录

日期：2026-10-07。本记录是提交前的本地验收快照，保留当时的测试结果与验收边界；不以本文说明远端推送、CI、发布或部署状态。

主仓基线：`f571d65268ed05fa919445950244e31cdf44c58b`；插件基线：`cfd1d5851f44df2287e24025f7f0ec0229dab668`。范围见 [开发计划](../plans/gamer_multiview_execution_plan.md)，操作说明见 [多画面使用指南](../guides/multiview.md)。

## 结论与验收边界

- 多画面、逐目标配置、独立运行及右侧目标标识均已实现；最终前端 aggregate3 **1442 项全部通过**，插件 UI/Web 构建及 SDK 检查通过，Rust Clippy、no-default-features、fmt 检查通过。右侧目标标识另有 **27/27** 定向测试通过。
- 真实 WASM 执行专项 **22 通过、1 忽略**，native sleep 专项 **2 通过**，RunManager 专项 **15 通过**。专项有重叠，不相加为唯一用例总数。
- 后端首次全套为 **1025 通过、5 失败、19 忽略**；TTL 修复后专项 **1/1 通过**，补齐市场夹具后的最终全套为 **1027 通过、3 失败、19 忽略**。只剩 3 项 ICE 候选环境受限失败，不能声明后端全套通过。
- **真实 4 路、9 路 Android、浏览器及混合目标均未完成验收**。组件、模拟 transport、合成帧和真实 WASM 执行测试各有边界，不等同于真实设备、实际浏览器 CDP 或 WebRTC/ICE 端到端通过。

## 已实现内容

- 默认 4 格、4/9 格切换、同目标去重、满 4 格加目标自动扩到 9 格；每目标独立 Android/WebRTC 或 browser/CDP 预览。
- 单套右侧面板跟随选中格，逐目标保存配置包、脚本和参数。“运行配置 → 保存配置”只保存，不运行；无参数脚本仍走显式保存流程，校验错误留在表单。
- 右侧增加“当前操作目标：名称”，无目标时提示先选画面，全局页面不显示。所选脚本运行日志与全局日志页保持不同范围，不将全局日志自动限定为当前目标。
- 右侧、格内与批量启动复用该目标的合法参数覆盖值，保留显式 `0` / `false`，不混用其他目标的旧参数建议。
- 首击未选中格只选择；切焦点释放旧触点、按键、映射和手柄输入，并静音旧画面。未保存编辑守卫拒绝切格时，焦点与面板保持原状。
- 批量启动/停止仅处理当前可见格，逐目标报告结果；停止确认捕获 `run_id`，隐藏、移除或替换后的实例不会被旧确认误停。放大时批量按钮禁用，单格操作保留。
- 关闭预览、移出网格、缩回 4 格或放大不停止后台脚本；刷新从服务端恢复运行状态，不自动启动新脚本。布局和运行配置保存在当前浏览器，不是服务端永久设备绑定或跨浏览器同步。
- 切换选中格退出原离线媒体展示并回实时；其他格不重连。媒体、模板裁切、匹配、录制与统计的迟到回包按目标/来源隔离；自动化识别原图不降质。
- WebRTC 异步连接回调按会话代次隔离；YAML 同步 WASM 执行与原生线程等待移入 blocking worker，复用 runtime bridge。普通 sleep 增加可取消等待，不缩短原时长或改变虚拟时钟契约。

## 本地验证结果

### 前端最终聚合

命令：仓库根目录执行 `pnpm --dir web test:run`。最终日志：`multiview-ui-aggregate-3.log`，退出码 0，包含右侧目标标识补充后的源码与测试。

| 范围 | 通过数量 | 说明 |
| --- | ---: | --- |
| Web 壳与集成测试 | 960 | 102 个测试文件 |
| YAML UI | 316 | 31 个测试文件 |
| Video UI | 21 | 5 个测试文件 |
| Package Publisher UI | 3 | 1 个测试文件 |
| Live UI | 14 | 3 个测试文件 |
| Notify UI | 3 | 1 个测试文件 |
| AI UI | 125 | 11 个测试文件 |
| 合计 | **1442** | 聚合命令通过 |

Keymap UI 测试入口无测试文件，以退出码 0 结束，不计入通过数量。旧基线 1372 项以及补目标标识前 aggregate2 的 1439 项不替代此轮最终结果。

多画面重点覆盖 `web/src/multiview-*.test.js` 与 WebRTC 生命周期回归：配置隔离与恢复、仅保存配置不执行、必填参数验证、`0` / `false` 保留、未保存编辑拒绝切换、输入释放与首击保护、4/9/放大布局、隐藏运行、批量快照、运行替换竞态、媒体/模板/录制/统计迟到回包及卸载清理。这些为单元、组件和集成夹具验证；没有实连 4/9 路目标。

右侧目标标识的定向测试 **27/27 通过**，包含新增的 3 项实际 Console 组件测试，验证目标名称/未选择提示与全局页面、全局日志的范围；随后 aggregate3 和 Web 构建复测均通过。所选运行日志与全局日志保持不同范围。

### 构建与静态检查

| 命令 | 结果 | 日志 |
| --- | --- | --- |
| 根目录：`node tools/build-plugin-ui.mjs` | 通过 | `multiview-plugin-build-2.log` |
| 根目录：`pnpm --dir web build` | 最终源码复测通过 | `multiview-web-build-3.log` |
| 根目录：`node tools/check-plugin-sdk.mjs` | 通过，宿主与固定插件 SDK 一致 | `multiview-sdk-check.log` |
| `server/`：`cargo clippy --locked --all-targets -- -D warnings` | 通过 | `multiview-clippy-1.log` |
| `server/`：`cargo check --locked --no-default-features` | 通过 | `multiview-no-default-check.log` |
| `server/`：`cargo fmt --all -- --check` | 通过 | `multiview-fmt.log` |

SDK 与 fmt 在最后源码上再次执行，退出码均为 0；最终 Git 差异空白检查通过。

本轮在 Linux 隔离环境执行。本环境的原生编译需要用 `taskset -c 0,1` 限制 `jpegli-sys` 可见 CPU；仅限制 Cargo jobs 不足以约束其 CMake 并行度。此为构建环境约束，不是产品运行配置，也不是其他主机的通用 CPU 编号。

### 执行专项

| 范围 | 结果 | 日志及边界 |
| --- | --- | --- |
| YAML 真实 WASM | 22 通过、1 忽略 | `yaml-wasm-regressions.log`；包含真实执行并发、HTTP 响应性及取消隔离 |
| native sleep | 2 通过 | `yaml-native-sleep.log`；覆盖原生等待时长、单个虚拟时钟等待与取消错误 |
| RunManager | 15 通过 | `yaml-run-manager-regressions.log`；覆盖不同目标并行、同目标互斥、取消、租约清理和终态等待 |

WASM 专项忽略项是 `old_yaml_component_is_rejected_before_any_host_input`，需要显式提供升级前组件 `GAMER_OLD_YAML_COMPONENT`，本轮未提供。真实 WASM 指真实 guest/component 执行，不代表真实 Android 游戏或浏览器画面已验证。

## 后端全套失败与复验

命令：在 `server/` 执行 `cargo test --locked -- --test-threads=2`。首次日志：`multiview-server-tests-1.log`，结果 **1025 通过、5 失败、19 忽略**。

| 首次失败项 | 原因与处理 | 当前复验状态 |
| --- | --- | --- |
| `official_plugin_market_end_to_end_with_committed_artifacts` | 缺少锁定的 `gamer-ai-0.3.10.gplugin` 市场夹具；已补齐与 CI `release/packaging/fetch-plugins.ps1 -TestFixturesOnly` 一致、按 release 锁校验哈希的已发布插件夹具，不放宽断言 | 最终全套中通过 |
| `ttl_boundary_preserves_base_files_and_retained_copy_and_survives_restart` | 旧测试硬编码 2026-10-05 完成日期，真实时钟已超过 TTL；仅测试改为相对当前时间登记，再显式检验 24 小时边界，生产保留逻辑未改 | 修复后专项 1/1 通过，日志 `multiview-ttl-retest.log`；最终全套中通过 |
| `gather_multi_ufrag_reuses_mux_with_concrete_addresses` | 当前受限环境没有收集到所需 ICE 本地候选 | 未通过，真实链路待验证 |
| `gather_via_agent_mux_produces_external_ip_candidates` | 当前受限环境没有收集到所需 ICE 本地候选 | 未通过，真实链路待验证 |
| `local_only_peer_gathers_usable_loopback_candidates_only` | 当前受限环境未生成可用的回环 ICE 候选 | 未通过，真实链路待验证 |

**最终后端复验：1027 通过、3 失败、19 忽略，耗时 250.65 秒，非全绿。** 日志为 `multiview-server-tests-2.log`；复用编译后的同一个 `gamer_server` 测试二进制直接执行 `--test-threads=2`，使测试进程不继承构建时的 `taskset` CPU 限制。剩余失败仅为上表的 3 项 ICE 候选环境问题。

市场测试临时切换过 registry，结束后已恢复原 `web/public/registry.json`：8522 字节，SHA-256 为 `e2d4b168da05b5dcf64a4e909a35fe9cc1aebaa6fa3b596ab650acf96da716a5`。该 registry 与 `release/plugins.lock.json` 的 Git diff/status 均为空，此次夹具切换未修改共享 UI。

市场测试验证已发布夹具的加载链路，不表示本轮修改后的插件已经打包或发布。专项或夹具复验通过不能覆盖剩余的 ICE 失败，忽略用例不计作通过。

## 尚未执行的真实验收

当前维持最多 9 路原质量、原分辨率预览，没有按格数自适应降帧。缩格或放大后隐藏目标仍保持连接。**真实 4/9 路的 CPU、内存、带宽、预览 FPS、延迟与长时间资源增长均未实测**，不能承诺任意设备组合下流畅运行；不通过降低自动化识别原图质量规避负载。

待在隔离数据和获准使用的真实目标上完成：

1. Android、browser/CDP 及混合 4 路/9 路连接、画面比例、viewer 冲突与接管、单路断线恢复。
2. 不同配置包、脚本和参数实际并发，长等待与短任务并行，取消一路后其他运行持续，核对日志与目标身份。
3. 鼠标、键盘、手柄、映射、音频及离线媒体切换；未保存编辑保护、模板原图裁切与真实匹配。
4. 关闭预览、放大、隐藏、重新添加与刷新后后台运行持续；记录原分辨率负载及连接/内存是否持续增长。

具体采样字段与安全要求见 [计划中的真实验收清单](../plans/gamer_multiview_execution_plan.md#五-风险与真实验收清单)。本报告不把未执行项目标为通过，不涵盖发布、推送或部署验证。
