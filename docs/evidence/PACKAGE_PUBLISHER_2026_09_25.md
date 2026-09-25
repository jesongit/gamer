# 配置包发布与公开仓库订阅验收

日期：2026-09-25。目标版本：Gamer 0.2.1 / gamer-package-publisher 0.1.0。

## 真实远端验收

用户指定并授权仓库 `git@github.com:jesongit/gamer-packages.git`（公开仓库）。空仓库初始化 README 后，通过实际安装、启用的 builtin 插件调用，使用本机 gh 登录创建并校验草稿、公开发布、重复草稿调用与重复公开调用。只上传无脚本、无个人数据的 `publisher-smoke`；不控制设备，不启动 Gamer 服务。

- Release：<https://github.com/jesongit/gamer-packages/releases/tag/catalog-20260925103634-14830bee>
- 配置：`publisher-smoke@1.0.0`，441 字节。
- SHA256：`52323a7a1095f98a1c293bca8d2bdddfffa9c9b90c10d9c05d7341f528307f3c`。
- 资产：packages.json、publisher-smoke-1.0.0.gamerpkg、SHA256SUMS.txt；远端逐项 size/digest 校验成功。
- 第一次闭环测试：创建草稿、重试草稿、公开、重试公开断言全部完成，但紧接着匿名发现未找到条目，测试最终失败。此失败保留在 `backups/package-publisher/live-release.log`，不记为完整通过。
- 随后不重复发布，执行独立只读 `publisher_live_subscription`：通过真实 PublicClient/PackageMarket 匿名发现、下载、SHA256/元数据核对和 extract_archive 导入；1 passed。日志 `backups/package-publisher/subscription.log`，warning=null、cached=false。
- 首次暂空具体响应未被旧测试打印，不能确定其根因；后续测试已打印发现结果以便定位。发布后读端可能短暂未同步，需刷新复核，不能因此重复发布。

## 本地自动化验证

- 前端全量：壳 781、YAML 257、视频 7、发布插件 2 测试通过；SDK 3 测试通过。后续新增发布权限提示测试，相关前端 8/8 通过。
- Rust package_ 定向：44 passed / 1 ignored；权限、生命周期、API 认证与仓库源 CRUD、归档校验、完整目录合并、版本冲突等。
- Rust clippy --all-targets -D warnings 通过。
- 发布插件独立构建、归档重读与 manifest/size/SHA256 自检通过。
- 插件发行脚本 3/3 测试通过；前端生产构建、版本一致性与 SDK 快照一致性检查通过。
- 全量 Rust、CI 与正式发行状态继续补充。

## 验收边界

以上是程序断言及实际 GitHub API/归档操作证据，没有将浏览器肉眼操作记为通过。此前 1920×1080 真机中心点击仍未完成；本轮没有重试被执行平台拒绝的设备/服务启动命令，也没有修改已公开 0.2.0 资产或既有安装目录。
