# 反馈窗口显示本次消耗

承接每条规则支持 0 消耗的功能，在反馈窗口的当前任务、最近结果和每个等待项显示「本次消耗 X 次」。`cost:0` 或任务入队时记录为 `billing_state:exempt` 时显示「免费」。已有的剩余可用和预占次数继续显示。

复用队列已公开的 `cost`／`billing_state`，按任务固定计费记录展示；不从可能后来修改的规则或账户状态推算。缺少有效数字时不误报免费。只修改宿主内嵌反馈 HTML，不修改计费、消息、队列调度和安装的插件载荷。

## 验证

- `pnpm --dir plugins/gamer-live/ui exec vitest run --maxWorkers=1 --no-file-parallelism`：60 项通过。新增覆盖当前和等待项的正数消耗、0 免费、免扣、从等待转为当前及最近结果，以及缺失或非法消耗字段。
- 独立浏览器在 320／420／1100 像素宽度检查当前任务、免费和最大规则消耗，无横向溢出，页面无 JS 异常。截图及回执在 `server/target/live-audience-cost-20261021/`；夹具不访问真实账号、设备或管理服务。
- Impeccable 机械检查无发现，固定 SDK 的 7 个文件一致。
- 初次并行运行前端和 Windows 宿主测试时，Node 内存分配失败，Rust 遇到系统错误 1455「页面文件太小」，该次宿主测试未成功执行。随后将前端改为单进程通过；没有改动系统内存设置或终止其他应用。本轮不把上一轮的 132 项宿主测试计作新增通过结果。

真实直播账号、观众消息和设备执行不在本轮验收范围。

## Docker 部署

- Linux 完整宿主 `release` 构建成功。镜像 `gamer:live-audience-cost-20261021`，镜像 ID `sha256:a38f0d823aa842ea12fa932e5b6e4a1d86996bf520788935502a1072d87de430`，未推送远端。
- 在独立空白卷、`--network none` 容器通过实际管理 API 及反馈 HTTP 页面检查：服务页面包含本次消耗、0 免费及任务免扣判断；原有 0 消耗规则保存校验、礼物入账账本格式和反馈状态检查继续通过。验收容器及专属空白卷已清理。
- 已更新本机 `gamer-release`，当前容器 ID `c4cb31cc7389667dafcd3c6550af048440cda2884d6581e01cbf1afa909f311c`，健康状态 `healthy`，`ready:true`。服务器 SHA256 `058dc3740ff0a78071ce3355f0cb4c3090cc99074b94ffbad1531290b1070ff4` 与验收镜像一致，并确认包含本次消耗页面实现。
- 沿用 `gamer-release-data`、18443 端口、原运行参数和环境。67 个既有保护文件逐字节保留，原数据库行、队列业务状态及插件生命周期状态一致，已安装 gamer-live 保持 0.2.16。本轮没有修改不可变安装载荷。
- 按此前用户要求没有备份原数据；旧容器保留为停止的 `gamer-release-previous-audience-cost-20261021`。直播、自动直播、输出及反馈开关保持原来的停止／关闭状态，无任务、设备和等待项。反馈网页继续使用原地址，刷新后加载新页面。

回执：`server/target/live-audience-cost-20261021/docker-qa.json`、`docker-deployment.json`；本机忽略的 `deploy/docker/local/release-runtime.json` 已更新。
