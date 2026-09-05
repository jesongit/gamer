// gamer.yaml 扩展（YAML 自动化 runner）的前端契约点：runner 注册 id 的唯一
// 配置源 + 统一执行入口（POST /api/runs）的该 runner 专用包装。
//
// 归属：ADR-11/13——runner 注册 id 是扩展知识，Core API 封装（api.js）只提供
// runner 无关的 api.run()；本模块与任务表单的 RunnerEditorContribution
// （components/task/builtin-runner-editors.ts）、Console 的 yaml 面板实现
// （components/console/useConsoleScriptRunner.js）同属 yaml 扩展前端侧。
import { api } from './api'

/** gamer.yaml runner 的注册 id（后端 YamlTimerRunner / run 分发目标）。 */
export const GAMER_YAML_RUNNER_ID = 'gamer.yaml'

/**
 * 运行一个 yaml 脚本：entrypoint = "<pkg>/<name>.yaml"（含 '/'，整体编码由
 * fetch 层承担）；payload = {args?, start_index?}——args 为稀疏显式覆盖映射，
 * 省略的参数由服务端按脚本 params 声明解析默认值。
 * 成功 202 {run_id, state, resolved_args}（与 api.run 相同的错误语义）。
 */
export function runYamlScript(id, deviceId, startIndex = 0, args) {
  return api.run({
    runner_id: GAMER_YAML_RUNNER_ID,
    entrypoint: id,
    device_id: deviceId,
    payload: {
      ...(startIndex ? { start_index: startIndex } : {}),
      ...(args && Object.keys(args).length ? { args } : {}),
    },
  })
}

/**
 * 函数测试运行：entrypoint = "<pkg>/<文件短路径>.yaml[#函数名]"（function
 * 缺省 = 文件第一个函数）；payload 语义与 runYamlScript 相同。
 */
export function runYamlFunction(id, deviceId, opts = {}) {
  return api.run({
    runner_id: GAMER_YAML_RUNNER_ID,
    entrypoint: opts.function ? `${id}#${opts.function}` : id,
    device_id: deviceId,
    payload: {
      ...(opts.start_index !== undefined ? { start_index: opts.start_index } : {}),
      ...(opts.args && Object.keys(opts.args).length ? { args: opts.args } : {}),
    },
  })
}
