# 配置仓库与发布

Gamer 0.2.1 起支持公开 GitHub 配置仓库。官方源为 `https://github.com/jesongit/gamer-packages`；添加、停用、移除仓库不会删除已安装配置。

## 下载配置

在“配置包”页填写 `https://github.com/owner/repo` 并添加；也接受 `git@github.com:owner/repo.git`。软件读取最新正式 Release 的 `packages.json`，显示包版本、适用应用、依赖插件与来源，下载前后校验归档大小和 SHA256，再使用原有导入机制。

下载者无需 GitHub 登录或 gh。私有仓库及其他代码托管站暂不支持。网络失败显示已校验的进程内缓存和错误提示；某个源失败不影响其他源。

同 ID 配置按来源分别显示，但本地 Package ID 仍唯一。安装已有 ID 会明确提示覆盖所有本地修改，确认前可先导出备份。安装配置不会自动运行，也不会自动安装依赖插件。

## 发布配置

安装并启用“配置包发布”插件。在运行 Gamer 的电脑上安装 GitHub CLI，通过 `gh auth login --hostname github.com` 登录，并创建有初始提交的公开仓库。发布账号需要仓库写权限。

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

该示意中的大小和摘要不是可安装值，实际由插件生成。目录最多 128 个包，总归档不超过 512 MiB；单包继承现有 100 MiB 归档限制。资源导出默认不包含媒体字节。目录不允许自定义外部下载 URL，资产只能来自所添加仓库的当前 Release。

仓库源保存在本机 `data/package-sources.json`；发布任务和冻结归档保存在 `data/package-publisher/jobs/`。首次无源设置时展示官方源，用户显式移除后不会重新添加。发布所用 gh 账号属于运行服务的电脑，不是访问网页的电脑。
