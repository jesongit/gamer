# 配置仓库与发布

Gamer 支持公开 GitHub、Gitea / Forgejo 配置仓库、HTTPS 前缀镜像和独立配置目录。官方源为 `https://github.com/jesongit/gamer-packages`；来源预设的更改不会删除已安装配置。

## 下载配置

在“配置包”页的“配置来源”下拉框选择 GitHub、内网或自己的预设。点击“管理预设”可添加、编辑或删除自定义来源；预设保存在服务端，当前选择由各浏览器分别记住。插件页使用相同方式，详情见 [来源预设](market-sources.md)。支持以下地址，通过实际读取 `packages.json` 校验目录格式：

- GitHub：`https://github.com/owner/repo`、`owner/repo` 或 `git@github.com:owner/repo.git`，读取最新正式 Release。
- Gitea / Forgejo：例如 `http://192.168.31.10:13000/posase/gamer-packages`，可带 `.git` 后缀。读取包含 `packages.json` 的最近正式 Release，并从该 Release 的实际附件地址下载。
- 前缀镜像：`https://mirror.example/https://github.com/owner/repo`，也支持 `https://mirror.example/cache/https://github.com/owner/repo`。先探测镜像的 Release 接口；仅支持附件下载的镜像则读取 `releases/latest/download/packages.json`。请求始终经过所填镜像。
- 独立目录：`https://mirror.example/configs/packages.json`，或目录 `https://mirror.example/configs/`（自动补 `packages.json`）。归档与目录 JSON 放在同一目录内。

软件显示包版本、适用应用、依赖插件与来源；下载时核对归档大小、SHA256 和包内元数据，再使用原有导入机制。来源须使用 HTTPS；私有或回环 IP 地址可使用 HTTP，不能含账号密码、查询参数或片段。下载者无需登录或 gh；首版不保存私有仓库凭据。网络失败显示当前来源的已校验缓存和错误提示，不会改用另一来源。

离线导入直接选择本地 `.gamerpkg` 文件。通过局域网 HTTP 访问 Gamer、浏览器未开放 WebCrypto 时，前端使用随应用打包的 SHA-256 实现继续校验，无需额外联网。

市场一次显示一个所选来源，本地 Package ID 仍唯一。安装已有 ID 会明确提示覆盖所有本地修改，确认前可先导出备份。安装配置不会自动运行，也不会自动安装依赖插件。

## 发布配置

发布插件仍以 GitHub 公开仓库为目标；下载镜像不是发布目标。安装并启用“配置包发布”插件。在运行 Gamer 的电脑上安装 GitHub CLI，通过 `gh auth login --hostname github.com` 登录，并创建有初始提交的公开仓库。发布账号需要仓库写权限。

1. 在插件面板选择本地配置，填写仓库 URL 与说明。已导出的 .gamerpkg 可先导入本地再选择。按需勾选包含媒体。
2. 生成发布预览，核对账号、仓库、全部配置版本和大小。旧目录中未更新的包会自动保留。
3. 创建 Release 草稿，上传冻结后的归档，核对全部资产。失败可从恢复任务中重试；相同内容跳过，不同内容拒绝覆盖。
4. 检查后点击“公开发布”。用户刷新仓库即可发现新目录。若其他作者已先发布，重新准备预览以合并最新目录。

包内版本采用正式 SemVer，如 `1.0.0`；同版本内容不可改变，不允许降级。Release 标签代表目录快照，与配置包自身版本独立。取消保留已创建的远端草稿，不会删除别人发布的内容。

## 仓库格式

每份正式 Release 包含 `packages.json`、`SHA256SUMS.txt` 以及清单内全部当前版本的 `.gamerpkg`。必须用附件中的归档，而不是 GitHub 自动生成的源码 ZIP。

```json
{
  "schema_version": 1,
  "packages": [{
    "id": "example", "name": "示例", "version": "1.0.0", "author": "作者",
    "android_targets": ["*"], "required_plugins": [], "optional_plugins": [],
    "asset_name": "example-1.0.0.gamerpkg", "size": 1234,
    "sha256": "此处为实际归档的64位小写SHA256"
  }]
}
```

该示意中的大小和摘要不是可安装值，实际由插件生成。目录最多 128 个包，总归档不超过 512 MiB；单包继承现有 100 MiB 归档限制。资源导出默认不包含媒体字节。目录不允许自定义外部下载 URL；GitHub 源的资产来自当前 Release，独立目录的资产来自 JSON 的同级路径，前缀镜像沿用其代理路径。目录与下载文件的大小、摘要或包内元数据不符时拒绝安装。

仓库源保存在本机 `data/package-sources.json`；发布任务和冻结归档保存在 `data/package-publisher/jobs/`。首次无源设置时展示官方源，用户显式移除后不会重新添加。发布所用 gh 账号属于运行服务的电脑，不是访问网页的电脑。
