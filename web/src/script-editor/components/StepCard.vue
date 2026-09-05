<template>
  <div
    class="step-card"
    :class="{
      selected, expanded, dragging, 'has-error': ownErrors.length > 0, 'card-highlight': highlighted,
      'drop-before': dropPosition === 'before', 'drop-after': dropPosition === 'after',
      [`kind-${step.kind}`]: true,
    }"
    :data-step-uuid="step.uuid"
    :data-step-path="stepPath"
    @click.stop="emit('select', step.uuid)"
    @dragover.prevent.stop="onDragOver"
    @dragleave.stop="onDragLeave"
    @drop.prevent.stop="onDrop"
  >
    <!-- 卡头：拖动手柄 + 图标 + 中文名 + 序号 + 摘要 + 动作按钮 -->
    <div class="card-head">
      <span
        class="drag-handle" title="拖动排序" draggable="true" role="button" aria-label="拖动排序"
        @dragstart.stop="onDragStart" @dragend.stop="onDragEnd" @click.stop
      >⋮⋮</span>
      <span class="kind-icon" :title="meta.hint">{{ meta.icon }}</span>
      <span class="kind-name">{{ meta.label }}</span>
      <span class="step-no">#{{ index + 1 }}</span>
      <span class="summary" :title="summary">{{ summary }}</span>
      <span v-if="ownErrors.length" class="err-badge" :title="ownErrors.map((d) => d.message).join('\n')">
        {{ ownErrors.length }}
      </span>
      <span class="head-actions">
        <!-- 函数测试入口：仅宿主开启 testFrom 时显示（函数库页签的函数体顶层卡片） -->
        <button
          v-if="testFrom" type="button" class="mini-btn test-from"
          title="从此步骤测试函数"
          @click.stop="emit('test-from', step.uuid)"
        >▶测试</button>
        <button
          type="button" class="mini-btn expand-btn"
          :title="expanded ? '收起' : '展开编辑'"
          @click.stop="onToggleExpand"
        >{{ expanded ? '▾' : '▸' }}</button>
        <button type="button" class="mini-btn" title="上移" :disabled="index === 0" @click.stop="moveBy(-1)">↑</button>
        <button type="button" class="mini-btn" title="下移" :disabled="index >= listLength - 1" @click.stop="moveBy(1)">↓</button>
        <button type="button" class="mini-btn" title="复制步骤" @click.stop="duplicate">⧉</button>
        <button type="button" class="mini-btn danger" title="删除步骤" @click.stop="remove">✕</button>
      </span>
    </div>

    <!-- 展开态：按类型的强类型控件（不提供任意键值编辑器） -->
    <div v-if="expanded" class="card-body" @click.stop>
      <div v-if="step.kind === 'app_start' || step.kind === 'app_stop'" class="field-hint">{{ meta.hint }}</div>
      <template v-if="step.kind === 'app_start' || step.kind === 'app_stop'">
        <div class="field-row">
          <label class="field-check">
            <input type="checkbox" :checked="step.package !== null" @change="togglePackage(($event.target as HTMLInputElement).checked)" />
            指定应用包名
          </label>
          <CellEditor
            v-if="step.package" :cell="step.package" type="expr" :params="params"
            label="应用包名" placeholder="com.example.app 或 $target" :error="fieldError('package')"
            @change="(c) => updateCell('package', c)"
          />
        </div>
      </template>

      <!-- tap -->
      <template v-else-if="step.kind === 'tap'">
        <div class="field-row">
          <span class="field-label">坐标</span>
          <CellEditor :cell="step.at" type="coord" :params="params" label="坐标" :error="fieldError('at')" @change="(c) => updateCell('at', c)" />
          <span class="field-hint">可切「引用」填 $reward.center 等匹配结果坐标</span>
        </div>
      </template>

      <!-- swipe -->
      <template v-else-if="step.kind === 'swipe'">
        <div class="field-row">
          <span class="field-label">起点</span>
          <CellEditor :cell="step.from" type="coord" :params="params" label="起点" :error="fieldError('from')" @change="(c) => updateCell('from', c)" />
        </div>
        <div class="field-row">
          <span class="field-label">终点</span>
          <CellEditor :cell="step.to" type="coord" :params="params" label="终点" :error="fieldError('to')" @change="(c) => updateCell('to', c)" />
        </div>
        <div class="field-row">
          <span class="field-label">时长</span>
          <CellEditor :cell="step.duration" type="time" :params="params" label="时长" :error="fieldError('duration')" @change="(c) => updateCell('duration', c)" />
        </div>
      </template>

      <!-- key -->
      <template v-else-if="step.kind === 'key'">
        <div class="field-row">
          <span class="field-label">按键</span>
          <CellEditor :cell="step.key" type="key" :params="params" label="按键" :error="fieldError('key')" @change="(c) => updateCell('key', c)" />
          <label class="field-check" title="press = 按下并抬起；down/up 用于长按场景">
            方式
            <select
              class="cell-input target-select" :value="step.action ?? 'press'" aria-label="按键方式"
              @change="setAction(($event.target as HTMLSelectElement).value)"
            >
              <option value="press">press</option>
              <option value="down">down</option>
              <option value="up">up</option>
            </select>
          </label>
        </div>
      </template>

      <!-- text -->
      <template v-else-if="step.kind === 'text'">
        <div class="field-row">
          <span class="field-label">文本</span>
          <CellEditor :cell="step.value" type="text" :params="params" label="文本" :error="fieldError('value')" @change="(c) => updateCell('value', c)" />
        </div>
      </template>

      <!-- log -->
      <template v-else-if="step.kind === 'log'">
        <div class="field-row">
          <span class="field-label">日志</span>
          <CellEditor :cell="step.message" type="text" :params="params" label="日志内容" multiline :error="fieldError('message')" @change="(c) => updateCell('message', c)" />
          <select
            v-if="step.level" class="cell-input target-select" :value="step.level" aria-label="日志级别"
            @change="setLevel(($event.target as HTMLSelectElement).value)"
          >
            <option v-for="lv in LOG_LEVELS" :key="lv" :value="lv">{{ lv }}</option>
          </select>
          <button v-else type="button" class="mini-btn" title="指定级别（缺省 info）" @click.stop="setLevel('info')">+ 级别</button>
        </div>
      </template>

      <!-- wait -->
      <template v-else-if="step.kind === 'wait'">
        <div class="field-row">
          <span class="field-label">时长</span>
          <CellEditor :cell="step.min" type="time" :params="params" label="时长" :error="fieldError('min')" @change="(c) => updateCell('min', c)" />
          <label class="field-check" title="启用后等待 min～max 内的随机值（{min, max}）">
            <input type="checkbox" :checked="step.max !== null" @change="toggleRandom" />
            随机区间
          </label>
          <CellEditor
            v-if="step.max" :cell="step.max" type="time" :params="params"
            label="随机上限" :error="fieldError('max')" @change="(c) => updateCell('max', c)"
          />
        </div>
      </template>

      <!-- find -->
      <template v-else-if="step.kind === 'find'">
        <div class="field-row">
          <span class="field-label">模板</span>
          <CellEditor :cell="step.template" type="tmpl" :params="params" :templates="templates" label="模板" :error="fieldError('template')" @change="(c) => updateCell('template', c)" />
        </div>
        <div class="field-row">
          <label class="field-check" title="未勾选时引擎默认等待 30min">
            <input type="checkbox" :checked="step.timeout !== null" @change="toggleFindTimeout" />
            等待超时
          </label>
          <CellEditor
            v-if="step.timeout" :cell="step.timeout" type="time" :params="params"
            label="超时" :error="fieldError('timeout')" @change="(c) => updateCell('timeout', c)"
          />
          <span v-else class="field-hint" title="未勾选时引擎默认等待 30min">默认 30min</span>
          <label class="field-check" title="本步匹配置信度覆盖 defaults.vision.threshold">
            <input type="checkbox" :checked="step.threshold !== null" @change="toggleThreshold('threshold', $event)" />
            匹配阈值
          </label>
          <input
            v-if="step.threshold !== null" class="cell-input num" type="number" min="0" max="1" step="0.01"
            :value="step.threshold" aria-label="匹配阈值"
            @change="setThresholdNum('threshold', ($event.target as HTMLInputElement).value)"
          />
        </div>
        <div class="field-row">
          <label class="field-check" title="把匹配结果 {found, score, center…} 存入变量，后续用 $名字.center 引用">
            <input type="checkbox" :checked="step.save !== null" @change="toggleSave" />
            保存结果
          </label>
          <input
            v-if="step.save !== null" class="cell-input" :value="step.save ?? ''" placeholder="变量名，如 reward"
            aria-label="保存变量名" @change="setSave(($event.target as HTMLInputElement).value)"
          />
          <label class="field-check" title="then 执行完后在超时内二次验证模板，失败按未命中处理">
            <input type="checkbox" :checked="step.verify !== null" @change="toggleVerify" />
            二次验证
          </label>
          <template v-if="step.verify">
            <CellEditor :cell="step.verify.template" type="tmpl" :params="params" :templates="templates" label="验证模板" :error="fieldError('verify.template')" @change="(c) => updateVerify({ template: c })" />
            <CellEditor
              v-if="step.verify.timeout" :cell="step.verify.timeout" type="time" :params="params"
              label="验证超时" :error="fieldError('verify.timeout')" @change="(c) => updateVerify({ timeout: c })"
            />
          </template>
        </div>
        <div class="field-hint">命中后用 $match.center 点击命中点；save 后可跨步用 $变量名.center</div>
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('then')" :base-path="subBase('then')"
          label="命中后" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
        />
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('else')" :base-path="subBase('else')"
          label="超时未命中" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
        />
      </template>

      <!-- match_first -->
      <template v-else-if="step.kind === 'match_first'">
        <div class="field-hint warn">按序首个命中分支获胜；命中候选执行自己的步骤组（可用 $match.center）</div>
        <div v-for="(cand, ci) in step.candidates" :key="ci" class="cand-block" :class="{ 'cell-error': !!fieldError(`candidates[${ci}].template`) }">
          <div class="field-row">
            <span class="field-label">候选 {{ ci + 1 }}</span>
            <CellEditor
              :cell="cand.template" type="tmpl" :params="params" :templates="templates"
              :label="`候选${ci + 1}`" :error="fieldError(`candidates[${ci}].template`)"
              @change="(c) => updateCandidateTemplate(ci, c)"
            />
            <label class="field-check" title="本候选举信度覆盖默认阈值">
              <input type="checkbox" :checked="cand.threshold !== null" @change="toggleCandThreshold(ci, $event)" />
              阈值
            </label>
            <input
              v-if="cand.threshold !== null" class="cell-input num" type="number" min="0" max="1" step="0.01"
              :value="cand.threshold" :aria-label="`候选${ci + 1}阈值`"
              @change="setCandThreshold(ci, ($event.target as HTMLInputElement).value)"
            />
            <button v-if="step.candidates.length > 1" type="button" class="mini-btn" title="删除候选" @click.stop="removeCandidate(ci)">✕</button>
          </div>
          <BranchContainer
            :model="model" :stack="stack" :container-path="candPath(ci)" :base-path="candBase(ci)"
            :label="candLabel(ci)" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
            :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
            :templates="templates"
            @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
            @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
          />
        </div>
        <div class="field-row">
          <button type="button" class="mini-btn add" title="添加候选" @click.stop="addCandidate">+ 添加候选</button>
        </div>
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('else')" :base-path="subBase('else')"
          label="都未命中" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
        />
      </template>

      <!-- check -->
      <template v-else-if="step.kind === 'check'">
        <div class="field-row">
          <span class="field-label">模板</span>
          <CellEditor
            :cell="step.template" type="tmpl" :params="params" :templates="templates"
            label="检查模板" :error="fieldError('template')" @change="(c) => updateCell('template', c)"
          />
          <span class="field-hint warn">检测期间不点击；超时未命中抛错结束运行</span>
        </div>
        <div class="field-row">
          <label class="field-check" title="未配置时引擎默认检测 5s">
            <input type="checkbox" :checked="step.timeout !== null" @change="toggleCheckTimeout" />
            检测超时
          </label>
          <CellEditor
            v-if="step.timeout" :cell="step.timeout" type="time" :params="params"
            label="检测超时" :error="fieldError('timeout')" @change="(c) => updateCell('timeout', c)"
          />
          <span v-else class="field-hint" title="未配置时引擎默认检测 5s">默认 5s</span>
          <label class="field-check" title="本步匹配置信度覆盖 defaults.vision.threshold">
            <input type="checkbox" :checked="step.threshold !== null" @change="toggleThreshold('threshold', $event)" />
            匹配阈值
          </label>
          <input
            v-if="step.threshold !== null" class="cell-input num" type="number" min="0" max="1" step="0.01"
            :value="step.threshold" aria-label="匹配阈值"
            @change="setThresholdNum('threshold', ($event.target as HTMLInputElement).value)"
          />
          <label class="field-check" title="自定义超时未命中的抛错文案（缺省「check 未命中」）">
            <input type="checkbox" :checked="step.throw !== null" @change="toggleCheckThrow" />
            抛错文案
          </label>
          <CellEditor
            v-if="step.throw" :cell="step.throw" type="text" :params="params"
            label="抛错文案" :error="fieldError('throw')" @change="(c) => updateCell('throw', c)"
          />
        </div>
      </template>

      <!-- set -->
      <template v-else-if="step.kind === 'set'">
        <div class="field-row">
          <span class="field-label">变量名</span>
          <input
            class="cell-input" :value="step.name" placeholder="变量名" aria-label="变量名"
            @change="setName(($event.target as HTMLInputElement).value)"
          />
          <span class="field-label">取值</span>
          <CellEditor :cell="step.value" type="expr" :params="params" label="取值" :error="fieldError('value')" @change="(c) => updateCell('value', c)" />
        </div>
      </template>

      <!-- if -->
      <template v-else-if="step.kind === 'if'">
        <div class="field-row">
          <span class="field-label">条件</span>
          <CellEditor :cell="step.cond" type="expr" :params="params" label="条件" :error="fieldError('cond')" @change="(c) => updateCell('cond', c)" />
          <span class="field-hint">布尔字面量、$flag 引用或任意表达式（按真值判断）</span>
        </div>
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('then')" :base-path="subBase('then')"
          label="如果为真" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
        />
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('else')" :base-path="subBase('else')"
          label="如果为假" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
        />
      </template>

      <!-- loop -->
      <template v-else-if="step.kind === 'loop'">
        <div class="field-row">
          <label class="field-check" title="省略次数 = 无限循环">
            <input type="checkbox" :checked="step.times === null" @change="toggleInfinite" />
            无限循环
          </label>
          <template v-if="step.times !== null">
            <span class="field-label">次数</span>
            <CellEditor :cell="step.times" type="number" :params="params" label="次数" :error="fieldError('times')" @change="(c) => updateCell('times', c)" />
          </template>
          <span v-if="step.times === null" class="field-hint warn">无限循环——请确保体内有 break 或其他退出条件</span>
          <span v-if="fieldError('steps')" class="cell-err-msg">{{ fieldError('steps') }}</span>
        </div>
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('steps')" :base-path="subBase('steps')"
          label="循环体" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
        />
      </template>

      <!-- call / invoke -->
      <template v-else-if="step.kind === 'call' || step.kind === 'invoke'">
        <div class="field-row">
          <span class="field-label">{{ step.kind === 'call' ? '目标' : '能力' }}</span>
          <template v-if="step.kind === 'call' && targetOptions">
            <select
              class="cell-input target-select" :value="step.target" aria-label="调用目标"
              @change="applyTarget(($event.target as HTMLSelectElement).value)"
            >
              <option value="">（选择目标）</option>
              <optgroup v-for="g in targetGroups" :key="g.id" :label="g.label">
                <option v-for="o in g.options" :key="o.target" :value="o.target">{{ o.label || o.target }}</option>
              </optgroup>
              <option v-if="step.target && !allTargets.some((o) => o.target === step.target)" :value="step.target">{{ step.target }}（已失效）</option>
            </select>
          </template>
          <input
            v-else-if="step.kind === 'call'"
            class="cell-input mono" :value="step.target"
            placeholder="script:daily/login 或 function:common/login"
            aria-label="调用目标"
            @input="setTarget(($event.target as HTMLInputElement).value)"
          />
          <input
            v-else
            class="cell-input mono" :value="step.capability"
            placeholder="能力名，如 vision.match"
            aria-label="能力名"
            @input="setCapability(($event.target as HTMLInputElement).value)"
          />
          <span v-if="fieldError(step.kind === 'call' ? 'target' : 'capability')" class="cell-err-msg">
            {{ fieldError(step.kind === 'call' ? 'target' : 'capability') }}
          </span>
        </div>
        <div class="field-row col">
          <span class="field-label">参数 with</span>
          <span v-if="targetOptions && step.kind === 'call'" class="field-hint">选定目标后按其声明自动生成（默认值已预填，必填项需补齐）；也可手动增删</span>
          <span v-if="fieldError('with')" class="cell-err-msg">{{ fieldError('with') }}</span>
          <div v-for="name in withNames" :key="name" class="arg-row">
            <input
              class="cell-input" :value="name" aria-label="参数名" placeholder="参数名"
              @change="renameArg(name, ($event.target as HTMLInputElement).value)"
            />
            <CellEditor
              :cell="step.with[name]" :type="argType(name)" :params="params"
              :templates="templates"
              :label="`with ${name}`" @change="(c) => updateArgValue(name, c)"
            />
            <button type="button" class="mini-btn" title="删除实参" @click.stop="removeArg(name)">✕</button>
          </div>
          <button type="button" class="mini-btn add" title="添加实参" @click.stop="addArg">+ 添加实参</button>
        </div>
        <div v-if="step.kind === 'call'" class="field-row">
          <label class="field-check" title="把调用返回值整体存入变量（被调方无 return 则存 null）">
            <input type="checkbox" :checked="step.save !== null" @change="toggleSave" />
            保存返回值
          </label>
          <input
            v-if="step.save !== null" class="cell-input" :value="step.save ?? ''" placeholder="变量名，如 result"
            aria-label="保存返回值变量名" @change="setSave(($event.target as HTMLInputElement).value)"
          />
        </div>
      </template>

      <!-- break -->
      <template v-else-if="step.kind === 'break'">
        <div class="field-hint warn">跳出最近一层 loop；只能放在 loop 子流程内</div>
      </template>

      <!-- throw -->
      <template v-else-if="step.kind === 'throw'">
        <div class="field-row">
          <span class="field-label">原因</span>
          <CellEditor :cell="step.message" type="expr" :params="params" label="终止原因" placeholder="原因文本或 $引用" :error="fieldError('message')" @change="(c) => updateCell('message', c)" />
          <span class="field-hint warn">会结束整个任务（含调用链）</span>
        </div>
      </template>

      <!-- return -->
      <template v-else-if="step.kind === 'return'">
        <div v-if="context !== 'function'" class="field-hint warn">return 在脚本中非法——只能出现在函数库（functions/）的函数体内</div>
        <div class="field-row">
          <span class="field-label">返回值</span>
          <CellEditor :cell="step.value" type="expr" :params="params" label="返回值" :error="fieldError('value')" @change="(c) => updateCell('value', c)" />
        </div>
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 步骤卡片（v3）：19 类全覆盖。
 * - 收起态 = 自然语言摘要（kinds.stepSummary）；
 * - 展开态 = 该类型强类型控件（无任意键值编辑器）；字段错误按 Diagnostic.field 标红定位；
 * - 左侧动作图标 + 中文名 + 序号 + 上移/下移/复制/删除（全部经 CommandStack）；
 * - find/match_first/if/loop 的分支子流程内嵌 BranchContainer（一层内嵌、更深专注）。
 * 纯受控组件：所有写操作构造 Command 提交 stack，自身不改模型。
 */
import { computed, inject, ref, watch, type PropType } from 'vue'
import type { Path } from '../commands'
import { resolveStepList } from '../commands'
import type { Diagnostic } from '../diagnostics'
import { joinStepPath } from '../diagnostics'
import { childContainerPath, containerLabel } from '../selection'
import type { Cell, CellType, ParamDecl, Step, StepKind } from '../model'
import {
  postRemovalIndex,
  clearActiveStepDrag,
  getActiveStepDrag,
  readStepDragPayload,
  writeStepDragPayload,
  type StepDragPayload,
} from '../step-dnd'
import { SE_TARGET_OPTIONS, type SeTargetOptions } from '../targets'
import { KIND_META, stepSummary } from './kinds'
import CellEditor from './CellEditor.vue'
import BranchContainer from './BranchContainer.vue'

/** 联合类型放宽：模板内按 kind 分支后访问各自字段；运行时由 v-if 保证形态。 */
type AnyStep = Step & Record<string, any>

const LOG_LEVELS = ['debug', 'info', 'warn', 'error']

const props = defineProps({
  model: { type: Object as PropType<Parameters<typeof resolveStepList>[0]>, required: true },
  stack: { type: Object as PropType<{ apply: (c: unknown, n?: string) => boolean }>, required: true },
  step: { type: Object as PropType<Step>, required: true },
  /** 宿主容器路径（move/duplicate/delete 按此寻址）。 */
  containerPath: { type: Array as PropType<Path>, required: true },
  /** step_path 字符串基（诊断定位）。 */
  basePath: { type: String, required: true },
  index: { type: Number, required: true },
  /** 0 = 根层卡片；1 = 一层内嵌分支内卡片。 */
  depth: { type: Number, default: 0 },
  diagnostics: { type: Array as PropType<Diagnostic[]>, default: () => [] },
  selectedUuid: { type: String, default: null },
  /** 诊断定位瞬态高亮的目标 uuid（区别于选中态；非本卡片时无效果）。 */
  highlightUuid: { type: String, default: null },
  /** 画布托管的展开集合；null（独立挂载）时用组件内部状态。 */
  expandedUuids: { type: Object as PropType<Set<string> | null>, default: null },
  context: { type: String as PropType<'script' | 'function'>, default: 'script' },
  params: { type: Array as PropType<ParamDecl[]>, default: () => [] },
  resolveTarget: {
    type: Function as PropType<((target: string) => { params: ParamDecl[] } | null) | undefined>,
    default: undefined,
  },
  templates: { type: Array as PropType<string[]>, default: () => [] },
  /** 显示「从此步骤测试函数」入口（函数库测试；仅函数体顶层容器由宿主开启）。 */
  testFrom: { type: Boolean, default: false },
})

const emit = defineEmits(['select', 'toggle-expand', 'focus', 'add-here', 'test-from'])

const meta = computed(() => KIND_META[props.step.kind as StepKind])
const summary = computed(() => stepSummary(props.step))
const stepPath = computed(() => joinStepPath(props.basePath, props.index))
const selected = computed(() => props.selectedUuid === props.step.uuid)
const highlighted = computed(() => props.highlightUuid === props.step.uuid)
const managedExpand = computed(() => props.expandedUuids !== null)
const localExpanded = ref(false)
const expanded = computed(() =>
  managedExpand.value ? (props.expandedUuids as Set<string>).has(props.step.uuid) : localExpanded.value,
)
function onToggleExpand(): void {
  emit('toggle-expand', props.step.uuid)
  if (!managedExpand.value) localExpanded.value = !localExpanded.value
}
const listLength = computed(() => resolveStepList(props.model, props.containerPath).length)
const ownErrors = computed(() => props.diagnostics.filter((d) => d.step_path === stepPath.value))

// ---------- 步骤拖放排序 ----------

const dragging = ref(false)
const dropPosition = ref<'before' | 'after' | null>(null)

function clearDropPosition(): void {
  dropPosition.value = null
}

function onDragStart(event: DragEvent): void {
  if (!event.dataTransfer) return
  const payload: StepDragPayload = {
    uuid: props.step.uuid,
    path: [...props.containerPath],
    index: props.index,
  }
  writeStepDragPayload(event.dataTransfer, payload)
  dragging.value = true
}

function onDragEnd(): void {
  dragging.value = false
  clearDropPosition()
  clearActiveStepDrag()
}

function dragPayload(event: DragEvent): StepDragPayload | null {
  return readStepDragPayload(event.dataTransfer) ?? getActiveStepDrag()
}

function onDragOver(event: DragEvent): void {
  const source = dragPayload(event)
  if (!source || source.uuid === props.step.uuid) {
    clearDropPosition()
    return
  }
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
  dropPosition.value = event.clientY < rect.top + rect.height / 2 ? 'before' : 'after'
  if (event.dataTransfer) event.dataTransfer.dropEffect = 'move'
}

function onDragLeave(event: DragEvent): void {
  const current = event.currentTarget as HTMLElement
  const next = event.relatedTarget
  if (next instanceof Node && current.contains(next)) return
  clearDropPosition()
}

function onDrop(event: DragEvent): void {
  const source = dragPayload(event)
  const position = dropPosition.value
  clearDropPosition()
  if (!source || !position || source.uuid === props.step.uuid) return
  const toIndex = postRemovalIndex(source, props.containerPath, props.index, position === 'before')
  props.stack.apply(
    {
      type: 'move_step',
      from: { path: source.path, index: source.index },
      to: { path: [...props.containerPath], index: toIndex },
    },
    '拖动步骤',
  )
}

function fieldError(field: string): string {
  return ownErrors.value.find((d) => d.field === field)?.message ?? ''
}

const s = computed(() => props.step as AnyStep)

// ---------- 命令提交 ----------

function updateStep(fields: Record<string, unknown>): boolean {
  return props.stack.apply({ type: 'update_step', path: [...props.containerPath, props.index], fields }, `编辑 ${meta.value.label}`)
}

function updateCell(field: string, cell: Cell): void {
  updateStep({ [field]: cell })
}

function moveBy(dir: -1 | 1): void {
  props.stack.apply(
    { type: 'move_step', from: { path: props.containerPath, index: props.index }, to: { path: props.containerPath, index: props.index + dir } },
    dir < 0 ? '上移步骤' : '下移步骤',
  )
}
function duplicate(): void {
  props.stack.apply({ type: 'duplicate_step', path: props.containerPath, index: props.index }, '复制步骤')
}
function remove(): void {
  const wasSelected = selected.value
  props.stack.apply({ type: 'remove_step', path: props.containerPath, index: props.index }, '删除步骤')
  if (wasSelected) emit('select', null)
}

// ---------- 分支子容器 ----------

function subPath(key: string): Path {
  return childContainerPath(props.containerPath, props.index, key, -1)
}
function subBase(key: string): string {
  return `${stepPath.value}.${key}`
}
function candPath(ci: number): Path {
  return childContainerPath(props.containerPath, props.index, 'candidates', ci)
}
function candBase(ci: string | number): string {
  return `${stepPath.value}.candidates[${ci}].steps`
}
function candLabel(ci: number): string {
  return containerLabel(props.step, 'candidates', ci)
}

// ---------- 各类型字段操作 ----------

function togglePackage(on: boolean): void {
  updateStep({ package: on ? { lit: '' } : null })
}
function setAction(v: string): void {
  updateStep({ action: v === 'press' ? null : v })
}
function setLevel(v: string): void {
  updateStep({ level: v === 'info' ? null : v })
}
function toggleRandom(e: Event): void {
  const on = (e.target as HTMLInputElement).checked
  if (on) {
    const cur = s.value.min
    updateStep({ max: cur && typeof cur.lit === 'string' ? { lit: cur.lit } : { lit: '1s' } })
  } else {
    updateStep({ max: null })
  }
}
function toggleFindTimeout(e: Event): void {
  updateStep({ timeout: (e.target as HTMLInputElement).checked ? { lit: '30s' } : null })
}
function toggleCheckTimeout(e: Event): void {
  updateStep({ timeout: (e.target as HTMLInputElement).checked ? { lit: '5s' } : null })
}
function toggleCheckThrow(e: Event): void {
  updateStep({ throw: (e.target as HTMLInputElement).checked ? { lit: '' } : null })
}
function toggleThreshold(field: string, e: Event): void {
  updateStep({ [field]: (e.target as HTMLInputElement).checked ? 0.85 : null })
}
function setThresholdNum(field: string, raw: string): void {
  const n = Number(raw)
  if (!Number.isFinite(n)) return
  updateStep({ [field]: Math.min(1, Math.max(0, n)) })
}
function toggleSave(e: Event): void {
  updateStep({ save: (e.target as HTMLInputElement).checked ? '' : null })
}
function setSave(v: string): void {
  updateStep({ save: v.trim() ? v.trim() : null })
}
function toggleVerify(e: Event): void {
  updateStep({ verify: (e.target as HTMLInputElement).checked ? { template: { lit: '' }, timeout: { lit: '5s' } } : null })
}
function updateVerify(fields: Record<string, unknown>): void {
  const cur = (s.value.verify ?? { template: { lit: '' }, timeout: null }) as Record<string, unknown>
  updateStep({ verify: { ...cur, ...fields } })
}
function setName(v: string): void {
  updateStep({ name: v.trim() })
}
function setCapability(v: string): void {
  updateStep({ capability: v })
}
function toggleInfinite(e: Event): void {
  updateStep({ times: (e.target as HTMLInputElement).checked ? null : { lit: 3 } })
}
function addCandidate(): void {
  updateStep({ candidates: [...s.value.candidates, { template: { lit: '' }, threshold: null, steps: [] }] })
}
function removeCandidate(i: number): void {
  updateStep({ candidates: s.value.candidates.filter((_: unknown, j: number) => j !== i) })
}
function updateCandidateTemplate(i: number, cell: Cell): void {
  updateStep({ candidates: s.value.candidates.map((c: unknown, j: number) => (j === i ? { ...(c as object), template: cell } : c)) })
}
function toggleCandThreshold(i: number, e: Event): void {
  const on = (e.target as HTMLInputElement).checked
  updateStep({ candidates: s.value.candidates.map((c: unknown, j: number) => (j === i ? { ...(c as object), threshold: on ? 0.85 : null } : c)) })
}
function setCandThreshold(i: number, raw: string): void {
  const n = Number(raw)
  if (!Number.isFinite(n)) return
  updateStep({ candidates: s.value.candidates.map((c: unknown, j: number) => (j === i ? { ...(c as object), threshold: Math.min(1, Math.max(0, n)) } : c)) })
}
function setTarget(v: string): void {
  void applyTarget(v)
}

// ---------- call 目标下拉（宿主经 provide(SE_TARGET_OPTIONS) 注入候选与解析） ----------

const targetOptions = inject<SeTargetOptions | null>(SE_TARGET_OPTIONS, null)
const allTargets = computed(() => targetOptions?.targets ?? [])
const targetGroups = computed(() => {
  const script = allTargets.value.filter((o) => o.target.startsWith('script:'))
  const fn = allTargets.value.filter((o) => o.target.startsWith('function:'))
  return [
    { id: 'script', label: '脚本（script:）', options: script },
    { id: 'function', label: '函数（function:）', options: fn },
  ].filter((g) => g.options.length > 0)
})

/**
 * 下发目标；宿主注入了解析器时一并按目标声明重生成 with（默认值预填、必填补类型空值），
 * 单条 update_step = 一次撤销。await 期间目标若又被改动则放弃（由最新一次变更接管）。
 */
async function applyTarget(next: string): Promise<void> {
  if (!next) {
    updateStep({ target: '', with: {} })
    return
  }
  if (!targetOptions) {
    updateStep({ target: next })
    return
  }
  const prev = String(s.value.target ?? '')
  let decls: ParamDecl[] | null = null
  try {
    decls = await targetOptions.resolveParams(next)
  } catch {
    decls = null // 解析失败不阻塞改目标：with 保持原样（校验层兜底）
  }
  if (String(s.value.target ?? '') !== prev) return
  try {
    updateStep(decls ? { target: next, with: argsFromDecls(decls) } : { target: next })
  } catch {
    // await 期间步骤已被删除（resolveStep 抛错）——放弃本次下发
  }
}

/** 按目标声明生成完整 with：有默认值填默认值；必填填类型空值待用户补。 */
function argsFromDecls(decls: ParamDecl[]): Record<string, Cell> {
  const out: Record<string, Cell> = {}
  for (const d of decls) {
    out[d.name] = { lit: d.default !== null && d.default !== undefined ? d.default : emptyLitFor(d.type) }
  }
  return out
}
function emptyLitFor(type: string): unknown {
  switch (type) {
    case 'boolean': return false
    case 'integer': case 'number': return 0
    default: return ''
  }
}

// ---------- with（call/invoke） ----------

const withNames = computed<string[]>(() => Object.keys(s.value.with ?? {}))

function argType(name: string): CellType {
  const target = String(s.value.target ?? '')
  // 宿主注入的同步缓存优先（实参类型需异步拉取后才有）；prop 解析器兜底
  const decls = targetOptions?.resolveParamsSync
    ? targetOptions.resolveParamsSync(target)
    : props.resolveTarget?.(target)?.params
  const t = decls?.find((d) => d.name === name)?.type
  switch (t) {
    case 'boolean': case 'bool': return 'bool'
    case 'integer': case 'number': case 'float': return 'number'
    case 'tmpl': return 'tmpl'
    case 'key': return 'key'
    case 'coord': return 'coord'
    default: return 'text'
  }
}
function updateArgValue(name: string, cell: Cell): void {
  updateStep({ with: { ...s.value.with, [name]: cell } })
}
function removeArg(name: string): void {
  const next = { ...s.value.with }
  delete next[name]
  updateStep({ with: next })
}
function renameArg(oldName: string, raw: string): void {
  const name = raw.trim()
  if (!name || name === oldName) return
  if (name in s.value.with) return // 重复键直接忽略（校验层另有 args_unknown 提示）
  const next: Record<string, Cell> = {}
  for (const [k, v] of Object.entries(s.value.with)) next[k === oldName ? name : k] = v as Cell
  updateStep({ with: next })
}
function addArg(): void {
  const args = s.value.with ?? {}
  let i = 1
  while (`param${i}` in args) i++
  updateStep({ with: { ...args, [`param${i}`]: { lit: '' } } })
}
</script>

<style scoped>
.step-card {
  position: relative;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-1);
  margin: 6px 0;
  overflow: visible;
}
.step-card.dragging { opacity: .45; }
.step-card.drop-before::before,
.step-card.drop-after::after {
  content: '';
  position: absolute;
  left: 6px;
  right: 6px;
  height: 3px;
  border-radius: 3px;
  background: var(--accent);
  box-shadow: 0 0 6px color-mix(in srgb, var(--accent) 70%, transparent);
  pointer-events: none;
  z-index: 2;
}
.step-card.drop-before::before { top: -5px; }
.step-card.drop-after::after { bottom: -5px; }
.step-card.selected { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }
.step-card.has-error { border-color: var(--danger); }
.step-card.card-highlight { border-color: var(--warn); box-shadow: 0 0 0 2px var(--warn); }

.card-head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 8px;
  cursor: pointer;
  min-height: 32px;
}
.card-head:hover { background: var(--bg-3); }
.drag-handle {
  color: var(--text-2); cursor: grab; font-size: 11px; letter-spacing: -2px;
  user-select: none; touch-action: none;
}
.drag-handle:active { cursor: grabbing; }
.kind-icon {
  display: inline-flex; align-items: center; justify-content: center;
  width: 20px; height: 20px; border-radius: 4px;
  background: var(--bg-3); color: var(--accent); font-size: 12px; flex: none;
}
.kind-name { font-weight: 600; font-size: 13px; white-space: nowrap; }
.step-no { color: var(--text-2); font-size: 11px; font-family: var(--mono); white-space: nowrap; }
.summary {
  color: var(--text-1); font-size: 12px;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; min-width: 0;
}
.err-badge {
  background: var(--danger); color: #fff; font-size: 10px; line-height: 1;
  border-radius: 8px; padding: 3px 6px; flex: none; cursor: help;
}
.head-actions { display: inline-flex; gap: 3px; flex: none; }
.mini-btn {
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 4px; font-size: 11px; padding: 2px 6px; cursor: pointer; line-height: 1.3;
}
.mini-btn:hover:not(:disabled) { color: var(--accent); border-color: var(--accent); }
.mini-btn:disabled { opacity: .35; cursor: not-allowed; }
.mini-btn.danger:hover:not(:disabled) { color: var(--danger); border-color: var(--danger); }
.mini-btn.add { color: var(--accent-2); }
.mini-btn.test-from { color: var(--accent); }
.mini-btn.test-from:hover { background: var(--accent); color: #06251c; }

.card-body { padding: 4px 10px 10px 32px; display: flex; flex-direction: column; gap: 4px; }
.field-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.field-row.col { flex-direction: column; align-items: flex-start; gap: 4px; }
.field-label { font-size: 12px; color: var(--text-2); min-width: 52px; flex: none; }
.field-check { display: inline-flex; align-items: center; gap: 4px; font-size: 12px; color: var(--text-1); cursor: pointer; }
.field-hint { font-size: 12px; color: var(--text-2); }
.field-hint.warn { color: var(--warn); }
.cell-err-msg { font-size: 11px; color: var(--danger); }
.cand-block {
  border-left: 2px solid var(--border);
  padding-left: 8px;
  margin: 2px 0;
  display: flex; flex-direction: column; gap: 4px;
}
.cand-block.cell-error { border-left-color: var(--danger); }
.arg-row { display: flex; align-items: center; gap: 6px; }
.cell-input.grow { flex: 1; }
.cell-input.num { width: 74px; }
/* .cell-input 基础样式在 CellEditor 的 scoped 块里，本组件的 select 拿不到——自包含补齐 */
.target-select {
  background: var(--bg-2); color: var(--text-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 3px 6px; font-size: 12px; min-width: 60px; max-width: 200px;
}
.target-select:focus { outline: none; border-color: var(--accent); }
.target-select option { background: var(--bg-1); color: var(--text-0); }
.field-sep { color: var(--text-2); font-family: var(--mono); }
.mono { font-family: var(--mono); }
</style>
