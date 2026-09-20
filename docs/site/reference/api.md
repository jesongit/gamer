# API 与 SDK 导航

Gamer 的 Web 界面通过 HTTP API 与 WebSocket 连接 Core，实时画面和输入使用 WebRTC。插件通过受权限约束的 SDK 使用宿主能力。

## HTTP API 分组

| 路径 | 主要用途 |
| --- | --- |
| `/api/devices` | 设备管理、扫描与控制 |
| `/api/packages` | 配置包、插件资源、导入导出 |
| `/api/runs` | 统一运行入口、查询与取消 |
| `/api/runners` | 执行器、入口 schema 与函数目录 |
| `/api/tasks`、`/api/task-presets` | 调度任务与预设 |
| `/api/media`、`/api/recording` | 媒体、帧索引与录制 |
| `/api/extensions` | 插件安装、生命周期、能力与 UI |
| `/api/logs` | 运行日志 |
| `/api/system` | 服务信息与托管更新状态 |
| `/ws/device/:id` | WebRTC 信令 |

业务 API 需要会话认证。写请求还需遵循同源检查；使用前端开发代理时不要随意改变 Origin 与 Host 的关系。

`POST /api/runs` 接受执行器、入口、参数、设备与配置上下文，成功受理返回 `202` 和 `run_id`。同一设备已有活动运行时返回冲突，不能以并发启动绕过设备运行守卫。

## 插件 Host API

WASM 插件的宿主契约定义在 `server/wit/`，能力调用由权限声明与宿主实现共同约束。配置资源寻址使用配置 ID、插件 ID、相对路径；保存时使用版本检查，避免覆盖其他编辑。

完整方法、权限、数据类型与 manifest 说明请查阅：

- [Host API 参考](https://github.com/jesongit/gamer/blob/main/docs/reference/PLUGIN_API.md)
- [插件开发指南](https://github.com/jesongit/gamer/blob/main/docs/guides/plugin-dev.md)
- [WIT 权威定义](https://github.com/jesongit/gamer/tree/main/server/wit)
- [独立 SDK 示例](https://github.com/jesongit/gamer/tree/main/sdk/examples)

接口与运行参数以当前服务端返回的 schema 为准。新增业务能力优先复用既有 capability 与插件公开动作，保持 Core 对具体脚本语言和项目格式无关。
