# 直播互动规则与队列验证

日期：2026-09-27（Asia/Shanghai）。用户在规则与队列讨论后以 `go` 授权实现。

## 交付

- Gamer 宿主 0.2.2；gamer-live 0.2.1，插件源码提交 `789d0820a3e8d13e5a498cb077508f4477ab5d53`。
- 新增规则与参数配置、6 个默认停用示例、预览／明确测试入队、100 项 FIFO、暂停／继续／取消／停止／移除／清空／重试、恢复核对与日志。
- 规则保存在配置包；队列持久化于本机 extension-data。Core 复用运行互斥，只新增通用参数绑定和 builtin shutdown 生命周期。
- 本地 8443 服务已重新编译启动，5173 前端保持运行。经认证的插件 update API 安装 0.2.1，返回 running；服务提供的 plugin.js 与最终构建文件逐字相同。
- 本地插件市场重建后 provenance 指向上述插件提交；未发布远端版本。

## 自动化验证

| 命令 | 结果 |
|---|---|
| `cargo test --locked --no-default-features --manifest-path server/Cargo.toml extensions::live -- --test-threads=2` | 18 通过 |
| 同上，过滤 `entrypoint_descriptor` | 3 通过 |
| 同上，过滤 `architecture_guard_tests` | 7 通过 |
| `pnpm --dir plugins/gamer-live/ui test` | 8 通过 |
| `cargo build --locked --manifest-path server/Cargo.toml` | 默认 WASM feature 宿主构建通过 |
| `node tools/check-plugin-sdk.mjs` | 7 文件固定快照与宿主一致 |
| `tools/check-version.ps1` | Cargo/web 均为 0.2.2 |
| `tools/build-plugins.ps1 -Plugin gamer-live` | 插件 UI、归档自检、市场生成通过 |

队列使用可控假 Runner 测试：前项终态和设备 Busy、暂停及移除竞争、取消未完成不推进、停用插件取消当前项、超时、换直播间、存储失败、离线、重启待核对、100 项容量、首匹配冷却、请求与平台 ID 去重、无 ID 不按正文误合并、冻结默认参数。官方格式礼物 `gift_num=5` 只生成一项并保留数量，gift_id 绑定为字符串。权限测试确认未获 run.submit/run.control 的调用不会到执行服务。

## 浏览器

1. Chromium 加载真实构建的组件、模拟 SDK API：绑定目标、添加示例、保存、预览不入队、测试入队、暂停／继续均通过。900×760、420×600、320×400 无横向溢出，内容可滚动且子页签固定，无 pageerror。
2. Chromium 通过短时本机代理加载**已安装插件**的 JS/CSS，连接真实 8443 API：读取设备、配置包、规则及队列状态成功，无 pageerror。代理仅在测试期间监听 loopback，凭据只在进程内用于请求，不写入浏览器或证据文件。
3. 自动化脚本首次尝试的精确 label 定位因 label 包含选项文本超时，改为限定目标区 select 定位后通过；属于测试定位问题。

截图位于本机 Codex 可视化目录的 `live-interaction-queue.png`，为模拟 SDK API 场景，不代表真实直播或设备运行。

## 真实 API 联调

创建独立临时配置包 `live-queue-qa-20260927`，写入仅含 log 的测试函数；没有修改用户原配置包。

- 明确绑定已有设备及临时包，保持队列暂停、直播触发关闭。
- 保存规则并取得资源版本；旧 expected_version 再保存被拒绝。
- 通过真实 gamer-yaml 参数绑定取得整数默认值 3，预览后 waiting=0。
- 相同 request_id 两次测试入队返回同一个 item_id，waiting=1、current=null、paused=true；项保留规则资源版本。
- 清空等待、解除绑定并删除临时配置包。最终 enabled=false、paused=true、target=null、current=null、waiting=0。历史中保留明确标识的已移除模拟测试项。
- 整个 API 测试未继续队列，没有提交 Runner 或操作设备。

## 未执行与首版限制

- 当前设备 API 状态 offline，无在线设备执行测试。真实函数的游戏动作及多项实机顺序尚未验收。
- 无真实 B 站账号凭据，本次未连接真实直播间，礼物连送仍需真实脱敏样本验证。协议契约测试与真实平台验证分开计算。
- 首版历史支持状态分页，等待项完整显示；观众搜索／历史按规则过滤暂未实现。无全局手动任务公平队列，不快照函数源码。
- 重启活动项保守进入待核对；文件损坏仅禁用队列，保留原文件。文件 16 MiB 预算可能早于 7 天／10000 条清理旧历史，不清理等待与待核对项。

产品使用说明见插件 `gamer-live/README.md`；早期方案与实施差异见 `docs/plans/gamer_live_interaction_v1.md` 第 9 节。
