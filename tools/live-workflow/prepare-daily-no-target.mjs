// Prepare a reviewed resource candidate. No device input, runs or notifications.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';

const root = 'D:/code/gamer';
const work = join(root, 'server/data/live-workflow-20261015/no-target-20261015');
const scriptId = '云游戏日常.yaml';
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const token = (await readFile(join(process.env.LOCALAPPDATA, 'gamer/admin-token'), 'utf8')).trim();
async function api(path, body) {
  const response = await fetch('http://127.0.0.1:8443' + path, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { 'X-Admin-Token': token, 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(30000),
  });
  const data = await response.json();
  assert(response.ok, `${path}: HTTP ${response.status}: ${JSON.stringify(data)}`);
  return data;
}
const resource = name => '/api/packages/hsr/plugins/gamer-yaml/resources/automations/' + encodeURIComponent(name);
const [formal, draft, outer] = await Promise.all([
  api(resource(scriptId)),
  api('/api/extensions/gamer-yaml/call', { action: 'authoring.get', values: { package_id: 'hsr', script_id: scriptId } }),
  api(resource('直播云游戏.yaml')),
]);
const head = draft.script.versions.at(-1);
assert.equal(head.yaml, formal.content, 'Preserve any unpublished user draft');
assert.equal(head.version, draft.script.published_version);
assert(!['generating', 'running', 'pending'].includes(draft.script.job?.state));
const local = await readFile(join(root, 'server/data/packages/hsr/plugins/gamer-yaml/automations', scriptId), 'utf8');
assert.equal(local, formal.content);
await mkdir(join(work, 'backup'), { recursive: true });
await writeFile(join(work, 'backup/daily.before.yaml'), formal.content, { flag: 'wx' });
await writeFile(join(work, 'backup/authoring.before.json'), JSON.stringify(draft.script, null, 2), { flag: 'wx' });
await writeFile(join(work, 'backup/live.before.yaml'), outer.content, { flag: 'wx' });

let yaml = formal.content.replaceAll('\r\n', '\n');
function once(source, before, after) {
  assert.equal(source.split(before).length - 1, 1, 'Expected exactly one edit anchor: ' + before.slice(0, 80));
  return source.replace(before, after);
}
const clickObstacle = 'daily_click_enter_obstacle_20261015#350_880_650_980.png';
const obsolete = `      - wait_find: {template: daily_click_enter_tight_20261011.png, click: true, timeout: 2h, region: [0.35, 0.88, 0.3, 0.1]}
        as: click_enter_hit
      - ne: {a: $click_enter_hit, b: null}
        as: click_enter_ok
      - if: $click_enter_ok
        then: []
        else:
          - fail: {message: '2h内未出现"点击进入"，未进入游戏', code: CLICK_ENTER_TIMEOUT}
`;
yaml = once(yaml, obsolete, '');
for (const [name, next] of [['cloud_enter_game', 'phone_resume_confirm'], ['phone_resume_confirm', null]]) {
  const start = yaml.indexOf(`  ${name}:\n`);
  const end = yaml.indexOf(next ? `  ${next}:\n` : '\nrun:\n', start);
  assert(start >= 0 && end > start);
  const section = yaml.slice(start, end);
  let updated = once(section, `            then: [{break: {}}]
            else: [{sleep: {duration: 2s}}]`, `            then: [{break: {}}]
            else:
              # 手机/月卡均未出现时清理一次点击进入障碍；每轮重新截图，支持需要多次点击的入口。
              - wait_find:
                  template: 手机#009_036_043_094.png
                  timeout: 0ms
                  threshold: 0.9
                  obstacles: [${clickObstacle}]
              - sleep: {duration: 2s}`);
  updated = updated.replace('循环内绝不点击/输入', '手机/月卡均未出现时才允许处理点击进入障碍');
  if (name === 'cloud_enter_game') {
    updated = once(updated, '      - repeat: 150', '      # 云端排队保留长等待余量：3600轮未命中间隔2s（非严格2h墙钟），仍受原解释器步数预算与取消约束。\n      - repeat: 3600');
    updated = once(updated, '150轮观察未检测到手机或月卡领取（每轮未命中间隔2s，非严格5m墙钟）', '3600轮观察未检测到手机或月卡领取（未命中间隔2s，非严格2h墙钟）');
  }
  updated = once(updated,
    'obstacles: [daily_supply_bottom_tight_20261011#440_840_560_890.png]',
    `obstacles: [daily_supply_bottom_tight_20261011#440_840_560_890.png, ${clickObstacle}]`);
  updated = updated.replace('第二次手机检查完整保留：该 wait_find 会先点击月卡障碍（唯一允许点击月卡处），再新截图确认手机',
    '第二次手机检查先清月卡（唯一允许点击月卡处），兼容迟到的点击进入，再从新截图确认手机');
  yaml = yaml.slice(0, start) + updated + yaml.slice(end);
}

const helper = `  enter_default_cavern:
    run:
      # 未设置培养目标：在左侧列表内向上拖动，找到底部的侵蚀隧洞类别。
      - repeat: 8
        do:
          - swipe: {from: [0.2833333333, 0.7527777778], to: [0.3078125, 0.2703703704], duration: 580ms}
          - wait_find: {template: daily_no_target_cavern_20261015.png, threshold: 0.9, timeout: 1s, interval: 250ms}
            as: cavern_hit
          - if: $cavern_hit
            then: [{break: {}}]
      - if: $cavern_hit
        then:
          - tap: $cavern_hit
        else:
          - fail: {message: '左侧列表滑动8次仍未找到侵蚀隧洞，停止导航', code: DEFAULT_CAVERN_CATEGORY_NOT_FOUND}
      - sleep: {duration: 1s}
      # 在右侧副本列表内向上拖动，直到末项收容舱段可见；原始录像为霜风之径。
      - repeat: 12
        do:
          - swipe: {from: [0.5682291667, 0.7620370370], to: [0.5885416667, 0.4092592593], duration: 460ms}
          - wait_find: {template: daily_no_target_storage_20261015.png, threshold: 0.9, timeout: 1s, interval: 250ms}
            as: storage_hit
          - if: $storage_hit
            then: [{break: {}}]
      - if: $storage_hit
        then: []
        else:
          - fail: {message: '右侧列表滑动12次仍未找到收容舱段，停止导航', code: DEFAULT_CAVERN_STAGE_NOT_FOUND}
      # 以地点文字的本次匹配位置限定同一行进入；录像中按钮中心比地点文字高23px。
      - calculate: {op: sub, a: $storage_hit.center.y, b: 0.0481481481}
        as: default_enter_top
      - wait_find:
          template: daily_no_target_enter_20261015.png
          click: true
          threshold: 0.9
          timeout: 5s
          region: [0.75, $default_enter_top, 0.1171875, 0.0629629630]
        as: default_enter
      - if: $default_enter
        then: []
        else:
          - fail: {message: '收容舱段同一行右侧的进入按钮未找到，停止导航', code: DEFAULT_CAVERN_ENTER_NOT_FOUND}

`;
yaml = once(yaml, '  drain_stamina:\n', helper + '  drain_stamina:\n');
yaml = once(yaml, `          # daily_train_target 点击用于切换左侧"培养目标"类别，与页面类型判断无关
          - tap_template: {template: daily_train_target_tight_20261011.png, region: [0.1, 0.2, 0.3, 0.4]}
          # 首项进入定位 ROI 收窄到第一项推荐副本按钮附近，避免命中其它同字"进入"
          - tap_template: {template: daily_enter_first_button_tight_20261011.png, region: [0.745, 0.48, 0.12, 0.11]}`, `          # 按直播流程约定，先检测下方添加培养目标；出现即走未设置目标的固定副本分支。
          - find: {template: daily_no_target_add_20261015.png, threshold: 0.9}
            as: no_training_target
          - if: $no_training_target
            then:
              - enter_default_cavern: {}
            else:
              # 已设置目标时沿用培养目标类别和首项推荐副本进入。
              - tap_template: {template: daily_train_target_tight_20261011.png, region: [0.1, 0.2, 0.3, 0.4]}
              - tap_template: {template: daily_enter_first_button_tight_20261011.png, region: [0.745, 0.48, 0.12, 0.11]}`);
yaml = yaml.replace(/^# 5\. 云入口路径：.*$/m,
  '# 5. 云入口点击进入游戏后，首次观察手机/月卡；二者未出现时反复处理点击进入障碍。月卡优先，手机先出现则等3s；随后等待手机并清理月卡及迟到的点击进入。');
yaml = yaml.replace(/^# 9\. 清体力分流：.*$/m,
  '# 9. 清体力分流：先检测添加培养目标；无目标则导航侵蚀隧洞→收容舱段同一行进入，有目标沿用首项推荐；统一确认挑战页后复用原战斗循环。');
await writeFile(join(work, '云游戏日常.candidate.yaml'), yaml, { flag: 'wx' });
await writeFile(join(work, clickObstacle), await readFile(join(root, 'server/data/packages/hsr/plugins/gamer-yaml/templates/daily_click_enter_tight_20261011.png')), { flag: 'wx' });
const summary = {
  script_id: scriptId, expected_resource_version: formal.version, expected_head_version: draft.script.head_version,
  previous_sha256: sha(formal.content), candidate_sha256: sha(yaml),
  preserved_outer_version: outer.version, preserved_outer_sha256: sha(outer.content),
  click_enter_obstacle: clickObstacle, original_daily_templates_unchanged: true,
  real_game: false, runs_started: false,
};
await writeFile(join(work, 'preparation.json'), JSON.stringify(summary, null, 2), { flag: 'wx' });
console.log(JSON.stringify({ ...summary, work }, null, 2));
