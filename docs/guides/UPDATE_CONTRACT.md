# 便携安装与更新契约

当前开发版移除独立启动器。`server` 在 Windows 链接 `updater` 库；同一应用承担正常启动、托盘和短时 `--update-worker` 工作进程。

```text
Gamer.exe                 稳定入口，与当前版本程序字节一致
versions/<version>/       gamer-server.exe、web-dist/、assets/
runtime/<id>/<version>/    清单固定的运行依赖与官方插件归档
manifests/<version>.json   schema_version = 2
config/config.toml         用户配置，更新时保留并备份
data/                     用户数据，离线快照
state/current.json        当前与上一版本指针
state/update-journal.json 持久事务阶段
state/apply-requested     手动更新预约，跨浏览器与服务重启保留
state/server/             服务实例锁
state/update/             更新进程实例锁
state/workers/            临时工作进程镜像
backups/                  数据及配置快照
logs/                     服务与恢复日志
```

应用包整体绑定前后端，安装切换只能选清单已锁定且已校验的候选。配置和数据不从发行包覆盖。新插件权限不静默批准。安装有独立维护屏障阻止新任务/录制/输出；旧进程实例锁释放后才开始离线快照。

HTTP：`POST /api/system/update/check` 检查，`POST /api/system/update/apply` 预约下载并安装，`DELETE /api/system/update/apply` 在安装前取消预约。`GET /api/system/update` 返回阶段、错误和 `apply_requested`。原 download/install 为内部受控动作。候选 activate/commit 使用本机更新令牌；更新控制不再经过命名管道。

完整流程和验证边界见 [UPDATE.md](UPDATE.md)。Manifest JSON Schema 为 `release/contracts/manifest-v2.schema.json`，要求 `minimum_updater_version`；旧 schema 不兼容。
