# Gamer 全插件正确性与交互优化：P6-WEB 最终验收证据

> 执行日期：2026-09-09
> 工作目录：`E:/code/gamer/web`
> 项目根目录：`E:/code/gamer`
> 审查基线：`b242c0d586c19bb0fdb996e05879e3f0459ef9ff`
> 验收状态：`PASS（自动化测试与构建）`

## 1. 验收范围

本轮前端验收覆盖当前工作树中的公共壳和三个官方插件：

- AddStep → Schema → Param/Form → YAML 往返；
- KeymapPanel、真实 context 适配、方案选择、保存、输入释放；
- PluginCenter 的版本关系、操作状态、失败提示和刷新保持；
- VideoWorkbench、MediaLibrary、VideoProjects、VideoDraft、VideoTimeline、TemplateStudio、Stage；
- 异步请求代次、Package/Android/Device context 分离、未保存保护、资源替换和媒体引用接线。

测试使用真实组件/组合式函数接线和故障注入夹具，不以手工改写旧字段绕过待修复转换逻辑。

## 2. 最终命令结果

### 2.1 Web 全量 Vitest

命令（工作目录 `E:/code/gamer/web`）：

```text
pnpm test:run
```

退出码：`0`

最终结果：

```text
Test Files  82 passed (82)
Tests       802 passed (802)
```

旧文档中记录的 3 个失败文件、5 个失败测试已经由最终集成测试收口；不再作为当前验收失败项。

覆盖重点包括：

- `script-editor` 正式测试及 Schema、步骤、参数回归；
- Keymap 面板、控制、运行时和正确性回归；
- PluginCenter 既有测试及 M02/M03 回归；
- Video 工作台、素材、录制事件、项目资源、草稿、时间轴、模板、Stage 和父装配集成回归；
- workspace context 分离、runner editor 和 Console 装配静态契约。

### 2.2 Web 正式构建

命令（工作目录 `E:/code/gamer/web`）：

```text
pnpm run build
```

退出码：`0`。Vite 生产构建完成并输出到 `server/web-dist/`。chunk 体积提示属于优化提示，不影响构建结果。

### 2.3 差异检查

命令（工作目录 `E:/code/gamer`）：

```text
git diff --check
```

退出码：`0`，未发现空白错误或冲突标记。

## 3. 链路判定

| 链路 | 自动化结果 | 判定边界 |
| --- | --- | --- |
| AddStep → Schema/Form | 通过 | 真实浏览器操作仍为 `NOT_VERIFIED` |
| Keymap | 通过 | 真实设备输入、按键释放和投屏标记仍为 `NOT_VERIFIED` |
| PluginCenter | 通过 | 真实后端部署及浏览器市场操作仍为 `NOT_VERIFIED` |
| VideoWorkbench / Media / Project / Draft / Timeline / Template / Stage | 通过 | 真实媒体、WebRTC 和设备流程仍为 `NOT_VERIFIED` |
| Context、请求过期、未保存保护、资源/引用接线 | 通过 | 测试夹具不替代跨进程或真实网络故障验证 |

## 4. 非阻塞运行噪声

测试期间仍可能看到以下环境噪声，但它们不改变 `82/82` 文件、`802/802` 测试的退出结果，也不是失败测试：

- happy-dom 的 `URL is not a constructor` 下载相关噪声；
- 本机 `127.0.0.1:3000` / `::1:3000` 连接拒绝噪声。

这些噪声已按非阻塞环境噪声保留，不写成业务链路通过依据，也不写成当前测试失败。

## 5. 必须保留的未验证项

以下项目没有在本轮自动化命令中完成，继续标记为 `NOT_VERIFIED`：

- 真实浏览器中的插件安装、启用、更新、卸载和完整 Video 工作流；
- 真实 Android/ADB 设备上的触控、按键释放、Keymap 应用和 stop_app 行为；
- 真实 WebRTC 视频/数据通道、live/media Stage 切换、跨页面接管；
- 真实后端部署下的 HTTP、Package 资源、媒体引用同步、录制服务和保存冲突；
- 实际 WASM/builtin 运行实例的跨进程恢复、更新/卸载端到端流程；
- 真实媒体样本的 VFR/PTS、录制分段、性能和平台兼容矩阵；
- 远端 registry/CDN、公开发布平台和远端下载安装链路。

## 6. 结论

P6-WEB 的自动化契约测试和正式构建已通过：`82/82` 测试文件、`802/802` 测试通过，`pnpm run build` 和 `git diff --check` 均退出码 `0`。上述真实环境项目不由本证据推导为通过，统一保留 `NOT_VERIFIED`。
