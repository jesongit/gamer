# Core 与官方插件 MCP 覆盖复核（2026-10-28）

本次对照 Core API、官方插件 manifest、外部处理器及原生动作检查可用工作流。范围是源码、本地构建和隔离测试，不部署服务，不操作用户设备，也不发布正式脚本或插件。

## 逐项结论

| 能力 | 现状与处理 |
| --- | --- |
| Core 目标、截图与控制 | 已有 targets / target_status / capture / control 工具；继续要求用户开始/恢复控制、租约和有效帧身份 |
| Core 配置包与资源 | 已有包清单和按插件隔离的列表/文本/PNG 读取；正式发布、替换和删除属于管理端 |
| Core 任务与运行证据 | 已有保存/控制、指定运行的状态/日志/Trace/图像；新增 schedule_preview 通过注册表校验并预览触发时间 |
| Core 原媒体与录制 | 前一轮已补七项只读发现/事件/帧接口；本轮保留对应回归 |
| Core OCR | 原有目标、Trace、Package PNG、素材帧；新增 media_frame 源，直接识别有权读取的原录像帧 |
| gamer-yaml | 18 项显式工具已覆盖语法、函数、工作稿、校验、素材匹配、裁图、私有试跑及历史；无需重复 Core 资源工具 |
| gamer-video | 保留 8 项素材工具，给 sample_list 增加名称搜索；创建素材时，空分段不能绕过媒体对象授权 |
| gamer-keymap | 原 4 项编辑工具缺少从空白编写的语法资料，新增 grammar，含当前限制和由权威解析器验证的示例 |
| gamer-live | 7 项工具已有目标绑定的状态/队列、规则读取/预览/保存和独立推流；不开放内部确认、扫码或全局配置动作 |
| gamer-notify | 通知是外部发送副作用；渠道私密配置和原始通知记录没有足够的设备/包对象绑定。本轮不自动导出 notification.send 或全局记录读取 |
| gamer-package-publisher | 准备、草稿、发布涉及归档写入和 GitHub 账号/发布状态，不自动转成 MCP 工具；仍由管理端操作 |
| 历史 gamer-ai / gamer-ocr | 不恢复退休执行能力；OCR 当前由 Core 提供，旧安装和私密数据保留 |
| 系统、原始全局日志、市场/升级 | 不把管理员 API 自动导出。全局自由文本日志不能保证对象隔离和凭据脱敏；任务诊断沿指定 run 的 journal/Trace 读取 |

“没有导出全部 REST/管理 action”不等于漏接。新增接口需形成具体可用链路，并保留显式 scope、实际对象范围和生命周期控制。

## 新增行为

- `gamer.core.schedule_preview`：`task.read`，最多十次未来触发，支持带偏移的起始时间。调用已注册 provider；未知 provider 不假报有效，非法计划与不推进的 provider 明确返回失败。仅预览，不保存或运行任务。UTC 与服务器本地时间分别返回。
- `gamer.core.ocr_recognize` 的 `media_frame`：要求 `ocr.recognize` + `media.read` + 媒体对象范围。通过实际媒体帧提取返回 PNG 与原始/工作几何信息，不接受路径或任意图片数据。取帧准备也纳入独立临时工作；请求断开不丢失清理责任，令牌撤销等待准备和识别工作实际收尾。
- `gamer.plugin.gamer-video.sample_list(name)`：按显示名称做中文/大小写不敏感包含搜索，授权与名称过滤后再计算总数和分页。
- `gamer.plugin.gamer-keymap.grammar`：显式只读声明，只要求已有 profiles.read scope 和插件授权，无需任意配置包或设备权限；不写资源、不激活输入。

视频源码版本为 0.2.7，键盘映射源码版本为 0.1.7。原生 host 代码需要随宿主更新，对应新增工具还需安装匹配插件声明；本记录不表示版本已公开发布。

插件源码提交：`c42de7d6c942111ea9a59ef6605085a5154a839a`，主仓固定 gitlink 同步到该提交。

## 验证与边界

测试覆盖调度注册表与无副作用、未知 provider/非法时间/条数上限、原录像帧的真实 PNG 与权限/Schema 拒绝、素材按名检索与授权后分页，以及映射语法示例通过当前解析器。所有素材为隔离夹具；原录像帧提取测试不调用真实 OCR 引擎或设备，不能当作真实文字识别效果验收。

- `cargo test --manifest-path server/Cargo.toml --bin gamer-server mcp`：48 项通过。
- 同一实际测试二进制的分组回归：`external` 50 项、架构边界 7 项、录制 27 项、映射 DSL 6 项、Cron 5 项通过；录制模块 4 项原有环境依赖测试保持忽略。分组有交叉，不累加为独立用例总数。
- `cargo clippy --manifest-path server/Cargo.toml --all-targets --all-features -- -D warnings`、Rust 格式与两仓 `git diff --check` 通过；`node tools/check-plugin-sdk.mjs` 确认固定 SDK 的 7 个文件一致。
- `cargo check --locked --manifest-path server/Cargo.toml --no-default-features` 通过，保留无 WASM 编译路径。
- 通过 `tools/build-plugins.ps1` 在 `server/target/mcp-audit-packages-20261028/` 隔离构建两款插件，保留其他归档；packer 的 ZIP、manifest、SHA256/size 自检通过。再次打开实际 ZIP 验证全部工具 Schema、新 grammar 声明和素材名称参数均已入包。
- 键盘映射 0.1.7 归档 SHA256：`4534c367ae1c26f29081c2ad526ed81a790b26fca7c150adbd097eee1a430341`。
- 视频 0.2.7 归档 SHA256：`8619c535a6b58a686d075bddfabf286188ead3893ce6662063a7cdaaa513cf36`。

用户的“退出云游戏”素材制作和正常/异常收尾脚本属于后续实际工作流，尚未因本次接口补齐而完成。
