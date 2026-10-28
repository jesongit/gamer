# MCP 正式发布、任务入口与插件更新（2026-10-29）

用户明确授权将这三类操作接入 MCP，覆盖历史“正式发布仅管理端”的限制。本轮为宿主 0.5.9 / gamer-yaml 0.3.28 源码准备，未发布发行资产、未更新实际安装、未操作真实设备。

## 接口与持久性

| 工具 | 准入与边界 |
| --- | --- |
| `gamer.plugin.gamer-yaml.draft_publish_preview` | 读取指定版本、head、源码及正式资源 SHA256，标明语法/引用检查范围，不执行设备输入 |
| `gamer.plugin.gamer-yaml.draft_publish` | 独立 publish scope；要求准确未发布 head、双 SHA256；复用发布事务和模板冲突检查，保留私有/正式历史 |
| `gamer.core.task_retarget` | 任务范围、时间戳和旧入口 CAS；检查新正式入口；保留时间表、启用状态、下一次触发、其他配置和历史；仅作用于未来提交 |
| `gamer.core.plugin_update_plan` | 已安装且获授权的插件；已配置来源；可下载真实归档预检，返回权限差异 |
| `gamer.core.plugin_update` | 旧/新版本与归档 SHA256 钉住；拒绝新增权限及执行类型改变 |
| `gamer.core.plugin_update_with_permissions` | 独立 `plugin.permissions.grant` scope；仅接受与真实归档完全一致的新增权限集合 |

副作用工具统一要求持久 `operation_id` 回执；同 ID、相同参数重试返回原结果。响应丢失或 unknown 时读取实际业务状态，不能换 ID 盲重放。

发布在同一配置包写屏障内复核正式资源，提交前复核令牌。受理后的发布完成原事务与历史写入；不增加通用管理 action、回退、删除或凭据配置入口。

启用任务的入口切换另需 `run.submit`，冻结当前令牌的受限执行策略，不继承管理员通知/推流授权。正在执行的运行仍持有原入口快照。

插件更新与运行提交共用空闲预约，存在活动运行即拒绝，新运行在预约期间也不能入场。插件调用门使用非阻塞写租约，不为更新取消在途调用；版本、权限与令牌在生命周期锁内再次检查，随后复用安装校验、状态恢复及失败恢复流程。旧不可变版本保留，禁用状态保留；管理端与 MCP 共用市场服务，来源修改会同步清空缓存。

## 首次升级

宿主及 YAML 插件需先更新一次。用户随后明确要求动态全部权限：设置页默认令牌及可识别的历史默认令牌升级后直接覆盖新增 scope，无需重建；显式受限令牌保持限制。重新读取工具目录后即可使用新功能。

## 验证

验证使用临时 SQLite、合成 Runner、离线市场与真实 ZIP/manifest/hash 检查；测试执行器拒绝真实设备 I/O。它们不代表生产部署、真实网络来源下载或游戏流程已经验收。

- Windows 后端完整测试：`cargo test --locked --manifest-path server/Cargo.toml --bin gamer-server`，1211 通过、0 失败、54 环境相关用例跳过，69.37 秒；设置 `ADB_MDNS=0`、`GAMER_PROFILE=dev`。
- 8 个新增用例覆盖双哈希/版本漂移及撤销拒绝、发布权限隔离、任务配置保留与 CAS、启用任务授权、归档与权限精确校验、禁用状态和不可变安装保留、回执重试、调用门与运行准入预约。
- `node tools/check-plugin-sdk.mjs`：固定宿主快照 `143309f17359e261ccace0827484c10f79ae6bb2` 的 7 个文件一致，无 SDK 接口变动。
- `cargo clippy --locked --manifest-path server/Cargo.toml --all-targets -- -D warnings`、Cargo 格式检查和新增 include 测试文件的独立 rustfmt 检查通过；主仓及插件仓 `git diff --check` 通过。
- 测试完整输出：`server/target/mcp-management-tests.log`（本地忽略产物，不随源码发布）。
