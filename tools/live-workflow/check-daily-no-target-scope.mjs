// Keep the shared routine's calculation, combat and notification paths intact.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';

const root = 'D:/code/gamer';
const work = join(root, 'server/data/live-workflow-20261015/no-target-20261015');
const { load } = createRequire(join(root, 'web/package.json'))('js-yaml');
const [before, candidate] = await Promise.all([
  readFile(join(work, 'backup/daily.before.yaml'), 'utf8'),
  readFile(join(work, '云游戏日常.candidate.yaml'), 'utf8'),
]);
const old = load(before), current = load(candidate);
assert.deepEqual(current.run, old.run, 'Keep the existing root routing');
assert.deepEqual(current.params, old.params, 'Keep notification defaults and public parameters');
assert.deepEqual(current.vars, old.vars);
for (const [name, original] of Object.entries(old.functions)) {
  if (!['cloud_enter_game', 'phone_resume_confirm', 'drain_stamina'].includes(name)) {
    assert.deepEqual(current.functions[name], original, 'Keep shared helper: ' + name);
  }
}
assert.deepEqual(current.functions.drain_stamina.run.slice(3), old.functions.drain_stamina.run.slice(3), 'Keep the entire existing stamina/OCR/challenge/combat/close flow');
const originalNavigation = old.functions.drain_stamina.run[2];
const navigation = current.functions.drain_stamina.run[2];
assert.deepEqual(navigation.then, originalNavigation.then, 'Keep already-in-challenge handling');
assert.deepEqual(navigation.else.slice(2), originalNavigation.else.slice(2), 'Keep common challenge-page confirmation');
assert.deepEqual(navigation.else[1].else, originalNavigation.else.slice(0, 2), 'Keep configured-target selection');
const report = {
  passed: true,
  candidate_sha256: createHash('sha256').update(candidate).digest('hex'),
  root_run_preserved: true, notification_params_preserved: true,
  other_functions_preserved: true, common_combat_preserved: true,
  challenge_confirmation_preserved: true, configured_target_navigation_preserved: true,
};
await writeFile(join(work, 'scope-validation.json'), JSON.stringify(report, null, 2));
console.log(JSON.stringify(report, null, 2));
