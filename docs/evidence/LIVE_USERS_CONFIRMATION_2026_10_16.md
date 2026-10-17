# 直播用户次数、本人确认、主动弹幕本地验收

用户已授权四项一起开发，先本地测试，真实直播账号和云游戏接入另验。正式 8443 服务不启停；正式脚本和历史队列暂不改写。

## 业务边界

- 入队原子预占，余额减预占不足不入队；默认新用户 1 次，可逐规则设正整数消耗，管理员可修改余额或设无限。用户身份绑定平台、应用、直播间、open_id，绝不用昵称代替。模拟用户单独隔离。
- 检测到进入游戏后等待本人“确认”，此时才起 5 分钟截止时间。“确认/取消”不收费，其他用户、提前、过期或重复控制不会变成普通规则任务。
- 取消和超时释放预占，独立收尾跳过开头重置及首页检测，直接退出登录。普通日常结束仍重置并退出。管理员停止不增加新的输入。
- 只有同一主运行、指定脚本来源的 `hsr_daily_final_reward` 原生视觉核验日志才正式扣次。失败无该证据则退回，核验后收尾失败仍为成功。日志读取不确定时保占并提示核对，不能靠超时冒充成功。
- 主动弹幕用独立 SESSDATA/bili_jct 配置；Windows DPAPI 本机保存，不返明文、不进入配置包。固定 HTTPS 目的地；本地假 HTTP 测试通过 cfg(test) 注入，生产不可改任意 URL。后台有界、限速、去重，无未知结果重发。发言失败不影响任务。固定 `【Gamer】` 前缀阻止自己回声触发任务。
- 网页反馈沿用原 8443 地址，不增加服务端口、不弹窗口。顶部只显示当前或最新结果一项，下方排队，含粉丝牌和完整 open_id。

## 本地文件与待验

候选脚本位于 `server/data/live-workflow-20261016/users-confirmation/`，基于正式 parent 5.0、daily 12.0、cleanup 1.0；仅父脚本和收尾脚本改变，日常不改。

本地验证已通过：

| 验证 | 结果 |
| --- | --- |
| `cargo test --locked extensions::live -- --test-threads=1` | 109 通过，含实际本机 HTTP 假平台、DPAPI、SQLite、权限与停止测试 |
| `cargo test --locked extensions::gamer_yaml -- --test-threads=1` | 196 通过、3 个需额外环境的测试按原定义忽略；新确认函数实际 WASM 桥测试通过 |
| `cargo test --locked architecture_guard_tests -- --test-threads=1` | 7 通过 |
| `pnpm --dir plugins/gamer-live/ui test` | 56 通过 |
| 唯一解释器 `--live-confirmation` 候选探针 | 23 场景通过，含原始候选父脚本、实际收尾 helper、模板唯一性/PNG 头/哈希和函数引用 |
| Impeccable 两轮视觉检查 | 最终 15 张截图、33 项断言通过；320/360 窄屏与宽屏、长身份、确认/运行/结果/空态均检查 |
| `node tools/check-plugin-sdk.mjs` | 主仓与固定 SDK 的 7 个文件一致 |
| 官方插件构建、自检 | gamer-live 0.2.14、gamer-yaml 0.3.19 均通过 |

原生 API 形状对照了 B 站[官方播放器文档](https://live.bilibili.com/p/html/bilibili-live-player/docs/player-h5.html)及其直接引用的播放器源码：固定 `msg/send`、POST 参数和 `bili_jct` 对应 CSRF。源码哈希和观察范围记录在 `server/target/live-users-confirmation-20261016/official-api-source.json`，这不能代替真实账号发言验收。

本轮审查同时修复旧确认消息被迟到采用、重启后真实领奖被误退款、旧手工成功接口绕过新收费判定、确认超时重复提醒，以及完整混合大小写 open_id 搜索问题。已全部加入对应回归。

## 产物与应用

验证日志、候选服务程序、两个 `.gplugin` 和 `SHA256SUMS` 位于 `server/target/live-users-confirmation-20261016/`；最终结构化摘要为同目录 `validation.json`。前端截图与视觉报告在 `server/target/live-quota-ui-20261016/`。候选脚本探针报告在数据候选目录的 `confirmation-probe.json`。

默认特性宿主已用 `cargo rustc --locked --bin gamer-server -- -o D:/code/gamer/server/target/live-users-confirmation-20261016/gamer-live-users-server.exe` 编译通过，实际输出 `gamer-live-users-server-a17d95d91559fd97.exe`，SHA-256 为 `122c99150035e061f819cc943417f9d7c53b6d3d2b9ebfaedd32b65240b964ca`；未启动该程序。配套插件归档的哈希分别为 `eded4c1a56acaf662a32a8401f36e8a8af362c139c141d1ab03d886f65d63380`（live）和 `4bfea9a76c5d7a0bf8f39888f31a91ca2d60d4a3b7d2a954102561b2d43b6e70`（yaml）。

`tools/live-workflow/apply-users-confirmation.mjs prepare` 已执行，只读核对正式版本并生成候选规则和发布计划，没有 API 写操作。`apply` 尚未执行：需要配套新宿主和两个插件，断开互动并停止输出、队列和设备运行后才会允许发布。工具预先核对全部草稿及正式版本，按收尾、主脚本、规则顺序发布；重试使用与对象键顺序无关的规则比较，不覆盖他人的草稿或规则。它不启动服务、不提交运行、不发设备输入。

正式 daily 12.0、parent 5.0、cleanup 1.0 保持原样。历史第 9 项仍为 success，未追溯创建计费。正在运行的 8443 服务未重启或替换。

真实平台鉴权、账号可发字数/频率、真实弹幕到达、真实扫码与退出、直播姬呈现均留待用户配合连接后验收。这里的 HTTP、WASM 和脚本模拟测试不代表这些真实效果已通过。

## 用户 rebuild 后的实际应用（2026-10-16）

用户明确授权应用配套插件和候选脚本后，已通过运行中的 8443 管理 API 完成更新，无需再次启停服务。先确认新宿主三个功能标志、直播断开、输出停止、队列及所有设备运行空闲，再按校验过的归档更新 gamer-yaml 0.3.19、gamer-live 0.2.14；安装后逐文件校验不可变载荷与归档相同，两者 Running 且无 last_error。

随后执行 `node tools/live-workflow/apply-users-confirmation.mjs apply`，发布收尾 2.0、直播主脚本 6.0 及配套规则，正式日常 12.0 字节保持不变。再次从 API 读取正式资源、发布版本、诊断、规则、函数目录和实际服务的 UI bundle，复核通过。历史第 9 项仍为 success，无追溯扣次。

原视频目录 `D:\code\gamer\live-videos`、设备“直播专用”、配置包 hsr 和两个本地素材 URL 保持原样；用户默认额度为 1。接收弹幕的已保存 OpenLive 档完整，发言账号尚未配置（主动弹幕关闭）。没有连接平台、发送消息或开始设备任务。可复查的数据为 `server/data/live-workflow-20261016/users-confirmation/plugin-deployment/result.json`、`deployment-result.json` 和 `deployment-verified.json`。

联调先刷新网页，沿用 OpenLive 已保存档连接接收；主动弹幕另填同一直播间和已登录发言账号的 SESSDATA、bili_jct 单项值。先手动发送短文字检查实际到达，再启动自动直播，用“日常”入队、扫码后本人“取消”验证退款和退出，随后用本人“确认”验证正式日常及最终领奖扣次。完整短测试规则仍关闭；正式规则是包含“日常”匹配，不能直接用“日常测试”来假定会跑短脚本。短测试若以后专门启用，自然成功使用该规则通用 cost 结算，与正式日常最终领奖 checkpoint 约束有区别，不能混淆这两种验收。
