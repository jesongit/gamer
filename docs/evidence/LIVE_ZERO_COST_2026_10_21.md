# 互动规则免费运行

后续反馈窗口补充了本次消耗及免费展示，当前 Docker 部署见 [反馈消耗展示证据](LIVE_AUDIENCE_COST_2026_10_21.md)。

本轮承接礼物入账和 Docker 部署，用户要求每次运行的消耗支持 0；此前已明确本机 Docker 为测试数据、无需备份。

## 行为

- 沿用每条互动规则的 `cost`，默认仍为 1，范围改为 0～1000000 的整数。设置位置为「直播助手 → 互动规则 → 编辑 → 每次消耗额度」。
- 0 表示免费：观众余额为 0 时仍可匹配和排队，不检查余额、不预占、不扣次，计费记录标记为 `exempt`。保持原有身份、匹配、冷却、队列及设备可用性条件；不会把观众账户永久改成不限次数。
- 已入队任务保留入队时消耗；手动重试保留原消耗。修改规则只影响新入队。正数规则原有成功扣次、失败或取消释放预占的行为保持。
- 管理界面、宿主校验和显式 MCP schema 一致支持 0。插件 UI/schema 使用新不可变版本 0.2.16；旧安装载荷保留。

## 已完成验证

- `cargo test --locked --no-default-features --manifest-path server/Cargo.toml extensions::live -- --test-threads=1`：132 项通过。新增覆盖零余额多观众、成功和失败免扣、重试及重启保留 0、移除免扣、切回正数恢复余额限制、原有付费任务不被改价豁免，以及数值边界。
- 同一测试程序的 `architecture_guard_tests --test-threads=1`：7 项通过。
- `pnpm --dir plugins/gamer-live/ui test`：58 项通过，包含 0 读取和保存，拒绝负数、小数、空值及超出上限，保留原有隐藏配置。
- 独立浏览器夹具在 375／1100 像素宽度完成读取 0、编辑并保存 0、键盘焦点和无横向溢出检查；无真实服务、账号或设备访问。截图和结果在 `server/target/live-zero-cost-20261021/`。
- Impeccable 机械检测无发现；固定 SDK 7 文件一致。`tools/build-plugins.ps1 -Plugin gamer-live -KeepStaleArtifacts` 构建及归档自检通过，0.2.16 归档 SHA256 `f852e53fa7aa0f2d534f0e129d8204345c3ecb67bde445f165b91c41fd09366e`。

上述属于本地与合成验证，不代表真实 B 站观众消息和设备执行已验收。

## Docker 部署结果

- 镜像 `gamer:live-zero-cost-20261021`，镜像 ID `sha256:9b90c4acf865584c92884b4fa3398948631f395782bd68e243c7307020903054`；本机源码构建，未推送远端。
- 先在全新专属测试卷、`--network none` 容器安装并启用 0.2.16，通过真实管理 API 保存并重新读取 `cost:0`，拒绝负数、小数和超过上限；实际服务的 UI 模块包含 0 下限和免费提示。旧礼物入账格式和反馈页面检查继续通过。测试容器和空白卷已清理。
- `gamer-release` 当前容器 ID `7d40cbe54d8369c73214747304128160e76117f710f90b1a0fce33c22004ee11`，健康状态 `healthy`，`http://127.0.0.1:18443/health/ready` 返回 `ready:true`；沿用 `gamer-release-data` 和原端口、环境、seccomp、共享内存、重启及日志设置。
- 服务端 SHA256 `3d0c1349096fdb4ae3ead02d9e71570cc0c64d84c0a35cbd8a1e95633f94174c` 与隔离验收镜像一致。通过正常插件更新 API 将 0.2.15 升级为 0.2.16，恢复 `running` 且无错误；实际加载的插件模块 SHA256 `a693d5e13466d66bf88ded2eb066f9e8a1531cc06cebb757dbb0bd22ed5224e6`。
- 原有配置、密钥及旧插件载荷等 57 个文件逐字节保留，原数据库行、队列业务状态、其他插件状态保持；旧 0.2.15 安装目录未重写。原容器保留为停止的 `gamer-release-previous-zero-cost-20261021`。
- 按用户要求没有备份或复制原数据卷。部署后直播连接断开、自动直播及输出停止，反馈网页保持关闭；无设备、任务、浏览器目标和排队项。未进行真实平台或设备操作。

结构化回执在 `server/target/live-zero-cost-20261021/docker-qa.json`、`docker-deployment.json` 和 `docker-plugin-update.json`，本机忽略的部署元数据 `deploy/docker/local/release-runtime.json` 已更新。
