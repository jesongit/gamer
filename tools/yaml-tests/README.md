# YAML V1 验收

这套用例把同一批 YAML 交给前端编辑器、真实 Rust REST、真实 WASM guest 执行，覆盖已暴露的保存与参数绑定问题。

```powershell
# 前端全部测试、服务端全部测试、解释器测试、前端生产构建
.\tools\yaml-tests\check.ps1
```

| 文件 | 覆盖 |
|---|---|
| `_function.yaml` | Package 函数参数、对象简写传参、嵌套调用、局部作用域、提前返回 |
| `flow.yaml` | 全部 11 种参数类型、默认值、false/0/null、嵌套引用、$$ 转义、if/repeat/return |
| `native.yaml` | 全部 18 个内置函数；输入、应用、日志、比较、单次匹配、轮询、点击、消失 |
| `native-functions.json` | 从正式注册表导出的表单 Schema；Rust 测试锁定一致性 |
| `live-api.mjs` | 真正安装插件，调用前端 API 与编辑器外壳：创建/保存/重开/改名/冲突/模板引用改写/任务 CRUD |
| `audit-packages.mjs` | 只读扫描指定 Package 根目录的全部自动化 YAML 和旧函数目录，输出逐文件诊断 |

前端测试还逐个挂载 18 个原生函数的参数表单，验证参数名为文本、按类型编辑、参数形态切换不丢值，并检查教程所有 YAML 示例。页面接线测试覆盖 Package 延迟加载和函数目录首次失败后的重试。

## 真实 API 与浏览器

先构建独立可执行文件，避免占用日常服务的可执行文件。默认测试端口 18443，数据、临时密码、日志和报告均写入被 Git 忽略的 `server/target/yaml-acceptance/`。

```powershell
cd server
New-Item -ItemType Directory -Path target/yaml-acceptance -Force | Out-Null
cargo rustc --bin gamer-server -- --emit=link=target/yaml-acceptance/gamer-qa-server.exe
cd ..
.\tools\yaml-tests\start-acceptance.ps1
node tools/yaml-tests/live-api.mjs

# 浏览器验收：新终端运行后打开 http://127.0.0.1:15173
cd web
$env:VITE_PROXY_TARGET = 'http://127.0.0.1:18443'
pnpm dev --port 15173 --host 127.0.0.1
```

以 admin 和 `server/target/yaml-acceptance/password.txt` 中的临时密码登录，选择 `live-api-report.json` 中的 Package，打开插件的自动化/函数/模板面板。依次验证新建、添加步骤、改值失焦自动保存、改名、保存后重开、原文编辑、函数增删和模板改名。内置参数名应为纯文本，脚本名输入框应无扩展名，运行摘要应与保存内容一致。

```powershell
# 结束隔离服务（校验 PID 对应的可执行文件，不影响日常服务）
.\tools\yaml-tests\stop-acceptance.ps1

# 只读检查日常 Package；有不合格文件时退出码为 1
node tools/yaml-tests/audit-packages.mjs
# 或指定实际数据目录
node tools/yaml-tests/audit-packages.mjs D:/your-data/packages
```

隔离实例禁用 ADB 扫描。`native.yaml` 的执行测试使用正式 WASM 解释器和可记录的 capability 测试实现，校验调用参数、返回值、事件和轮询次数；不会操作日常设备。真机输入、采集和画面匹配需要在测试 Android 设备上另做验收，不能用这组测试宣称真机通过。旧语法和旧 `functions/` 目录按 V1 契约报错，不做兼容或迁移。
