# 配置包适用目标

配置包的适用范围由 Core 解析、校验和匹配。声明不参与资源寻址，也不授予插件权限；不兼容只提示，不阻断运行或定时任务。

```toml
[targets.android]
packages = ["com.miHoYo.hkrpg"]

[targets.web]
url_prefixes = ["https://sr.mihoyo.com/cloud"]
```

至少填写一个 Android 包名或网页前缀。可以只声明其中一种，也可以同时声明；多项之间为“或”。Android 包名精确匹配，区分大小写；`*` 只匹配 Android。网页规则独立于 CDP、窗口采集等输入输出技术。

网页前缀必须是完整 HTTP(S) 地址，不能包含账号密码、查询参数或片段。匹配时比较协议、主机、有效端口和路径边界：`/cloud` 匹配 `/cloud`、`/cloud/play`，不匹配 `/cloud-other` 或其他域名。实际页面网址的查询参数和片段不参与匹配；需要限定这些条件的规则暂不支持，保存时明确报错。

`GET /api/targets/:id/identity` 只读目标身份，不启动目标。Android 读取设备配置的包名；网页在线时读取明确绑定的标签页，并校验会话 epoch/revision。未连接时返回配置网址，标记为 `configured` 预估；已连接目标失效或无法读取时返回 `unavailable`，不借用活动标签页或启动网址。

`GET /api/packages/:pkg/compatibility?target_id=...` 返回 `status`（`match` / `mismatch` / `unknown`）、`compatible`（true / false / null）、`reason` 和 `target`（kind/value/source/note）。手动预估可改用 `android_package` 或 `url` 参数，一次只传一种。结果是查询时快照；导航或切换目标后应重新检查。

新建、编辑、复制、归档导入导出和配置市场统一保留两类声明。市场目录中 `web_url_prefixes` 为可选数组，旧目录缺省为空。发布插件沿用 Core 的归档元数据提取，不另外实现匹配逻辑。

不再把空声明当通用配置。已有空声明需补充明确目标后再导入或使用；通用 Android 工具包可填写 `*`。用户数据不会通过启动迁移被自动扩大适用范围。
