# 直播素材与观众反馈固定地址（2026-10-15）

直播助手 0.2.11 的本机 HLS 素材和观众反馈网页复用本体 HTTP 监听端口（默认 8443），不再为两者创建临时端口。Core 只提供通用的 GET/HEAD `/local/extensions/:id/*path` 与扩展生命周期读租约；路径、独立读取密钥、展示开关及状态投影仍由插件负责。

`extension-data/gamer-live/local-links.json` 持久保存两个独立的读取密钥。同一数据目录和端口下，输出启停、反馈开关、插件重启和本体重启均沿用地址。损坏配置报错并保留原文件，不静默重置。当前工作区已预置这一文件，未开启正式输出或观众反馈。

关闭时两个地址仍可在设置中复制。反馈关闭拒绝读取；输出未启动、准备中或停止时素材入口返回带重试提示的 503。只允许当前输出的清单与数字 `.ts` 文件，保持 `epoch_us` 段编号，避免重新启动后读取旧片段。反馈页改为相对 `state` 请求，保留上一轮紧凑样式和状态字段白名单。

## 已完成验证

- `cargo test --locked --no-default-features extensions::live -- --test-threads=1`：54 项通过，包含固定地址持久化、坏文件保护、反馈开关、文字版本和原有 FIFO 回归。
- `cargo test --locked --no-default-features local_http -- --test-threads=1`：6 项通过，包含实际 TCP 路由的无管理登录读取、HEAD、方法限制、来源校验及生命周期租约。
- `cargo test --locked --no-default-features media::output -- --test-threads=1`：6 项通过、10 项按外部工具条件忽略。
- 显式运行 `repeated_local_starts_keep_the_host_url -- --include-ignored --test-threads=1`：真实 FFmpeg 生成本地合成视频，两次启停保持固定 URL，读取新片段并拒绝旧片段，1 项通过。
- `architecture_guard_tests`：7 项通过。
- 插件 UI：6 个文件、41 项测试通过；生产 UI 构建通过。
- `node tools/check-plugin-sdk.mjs`：宿主与固定 SDK 快照一致。
- `node --check tools/live-workflow/audience-api-qa.mjs` 与插件 `git diff --check` 通过。

## 产物与边界

单独命名的本体候选位于 `server/target/live-shared-address-20261015/gamer-shared-live-server-2d80183f3afa5636.exe`，SHA-256 为 `4215ab5b1d094eafe1528a0f83efa45d8476b1cae2dd3f204db3d7fc60cdc8db`。默认功能构建成功，未覆盖当前运行的本体。

插件包位于 `server/target/live-shared-address-20261015/plugins/gamer-live-0.2.11.gplugin`，SHA-256 为 `26952500738ef7130ab2734e2b895ceb3068a407717b205abeabb3043c1678a9`；打包、ZIP 回读、manifest 与哈希自检通过。反馈 HTML SHA-256 为 `b5608aad78da2221bac6467e1e35007c86ddb987984c41148b80eab1907976ff`。

安装包级验收脚本已更新，覆盖同端口实际 HLS/网页、无 Cookie Chrome 读取、启停同地址及监听端口检查，但本轮未执行：自动审批拒绝启动独立测试服务，返回 `blocked by policy`，未给出更具体原因。没有通过其他方式重试启动，也没有操作当前 8443 服务。因此不声称安装包级联调、实际监听端口观察或直播姬实测已通过；本体完整进程重启稳定性由持久文件重建测试支撑，未实际重启正式服务。

生效需要重建本体并更新直播助手至 0.2.11，再启用输出／反馈。旧的随机地址需更换为这次固定地址一次。

## 仅提供网页服务（0.2.12）

用户明确反馈页由直播姬的浏览器／网页素材加载。确认现有后端 `audience.open` 仅启用 HTTP 读取，没有自动打开浏览器或独立窗口；设置中的手动「预览网页」链接已移除，保留开关、固定 URL 复制和提示文字。固定链接与主端口不变，后端行为未改。

UI 41 项测试、生产构建及插件包自检通过。新包为 `server/target/live-feedback-service-20261015/plugins/gamer-live-0.2.12.gplugin`，未覆盖前述 0.2.11 验证产物。未启动浏览器或新服务，也未更新正在运行的正式实例。
