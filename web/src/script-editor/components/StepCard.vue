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
        <!-- 函数测试入口（阶段 5）：仅宿主开启 testFrom 时显示（函数库页签的函数体顶层卡片） -->
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
      <div v-if="step.kind === 'str_app' || step.kind === 'cls_app'" class="field-hint">{{ meta.hint }}</div>

      <!-- tap -->
      <template v-else-if="step.kind === 'tap'">
        <div class="field-row">
          <span class="field-label">坐标</span>
          <CellEditor :cell="step.at" type="coord" :params="params" label="坐标" :error="fieldError('at')" @change="(c) => updateCell('at', c)" />
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
          <CellEditor :cell="step.time" type="time" :params="params" label="时长" :error="fieldError('time')" @change="(c) => updateCell('time', c)" />
        </div>
      </template>

      <!-- key -->
      <template v-else-if="step.kind === 'key'">
        <div class="field-row">
          <span class="field-label">按键</span>
          <CellEditor :cell="step.key" type="key" :params="params" label="按键" :error="fieldError('key')" @change="(c) => updateCell('key', c)" />
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
        </div>
      </template>

      <!-- wait -->
      <template v-else-if="step.kind === 'wait'">
        <div class="field-row">
          <span class="field-label">时长</span>
          <CellEditor :cell="step.duration" type="time" :params="params" label="时长" :error="fieldError('duration')" @change="(c) => updateCell('duration', c)" />
          <label class="field-check" title="启用后等待 [时长, 上限] 内的随机值">
            <input type="checkbox" :checked="step.duration_max !== null" @change="toggleRandom" />
            随机区间
          </label>
          <CellEditor
            v-if="step.duration_max" :cell="step.duration_max" type="time" :params="params"
            label="随机上限" :error="fieldError('duration_max')" @change="(c) => updateCell('duration_max', c)"
          />
        </div>
      </template>

      <!-- find -->
      <template v-else-if="step.kind === 'find'">
        <div class="field-row">
          <span class="field-label">主模板</span>
          <CellEditor :cell="step.template" type="tmpl" :params="params" :templates="templates" label="主模板" :error="fieldError('template')" @change="(c) => updateCell('template', c)" />
          <label class="field-check" title="点击后等 interval；再重匹配，仍命中补一击">
            <input type="checkbox" :checked="step.verify" @change="setVerify(($event.target as HTMLInputElement).checked)" />
            二次确认
          </label>
        </div>
        <div v-for="(b, bi) in step.block" :key="bi" class="field-row">
          <span class="field-label">障碍 {{ bi + 1 }}</span>
          <CellEditor :cell="b" type="tmpl" :params="params" :templates="templates" :label="`障碍${bi + 1}`" :error="fieldError(`block[${bi}]`)" @change="(c) => updateBlockAt(bi, c)" />
          <button type="button" class="mini-btn" title="删除障碍" @click.stop="removeBlock(bi)">✕</button>
        </div>
        <div class="field-row">
          <button type="button" class="mini-btn add" title="添加障碍" @click.stop="addBlock">+ 添加障碍</button>
          <label class="field-check" title="未勾选时引擎默认等待 30min">
            <input type="checkbox" :checked="step.timeout !== null" @change="toggleFindTimeout" />
            等待超时
          </label>
          <CellEditor
            v-if="step.timeout" :cell="step.timeout" type="time" :params="params"
            label="超时" :error="fieldError('timeout')" @change="(c) => updateCell('timeout', c)"
          />
          <span v-else class="field-hint" title="未勾选时引擎默认等待 30min">默认 30min</span>
        </div>
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
          label="未命中" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
        />
      </template>

      <!-- match -->
      <template v-else-if="step.kind === 'match'">
        <div class="field-hint warn">按序首个命中分支获胜；候选可勾选「命中点击」，点中该模板框中心后等待 interval</div>
        <div v-for="(cand, ci) in step.candidates" :key="ci" class="cand-block" :class="{ 'cell-error': !!fieldError(`candidates[${ci}].template`) || !!fieldError('candidates') }">
          <div class="field-row">
            <span class="field-label">候选 {{ ci + 1 }}</span>
            <CellEditor
              :cell="cand.template" type="tmpl" :params="params" :templates="templates"
              :label="`候选${ci + 1}`" :error="fieldError(`candidates[${ci}].template`) || fieldError('candidates')"
              @change="(c) => updateCandidateTemplate(ci, c)"
            />
            <label class="field-check" title="命中后点击该候选模板匹配框的中心，并等待 interval（find 的点击语义）">
              <input type="checkbox" :checked="cand.click" :aria-label="`命中点击${ci + 1}`" @change="setCandClick(ci, ($event.target as HTMLInputElement).checked)" />
              命中点击
            </label>
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
          <label class="field-check" title="未配置超时时仅检测一轮：全未命中立即进「都未命中」分支">
            <input type="checkbox" :checked="step.timeout !== null" @change="toggleMatchTimeout" />
            轮询超时
          </label>
          <CellEditor
            v-if="step.timeout" :cell="step.timeout" type="time" :params="params"
            label="超时" :error="fieldError('timeout')" @change="(c) => updateCell('timeout', c)"
          />
          <span v-else class="field-hint" title="未配置超时时仅检测一轮：全未命中立即进「都未命中」分支">未配置仅检测一轮</span>
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
          <span class="field-hint warn">检测期间不点击；超时未命中结束运行</span>
        </div>
        <div class="field-row">
          <label class="field-check" title="未配置时默认检测 5s；timeout=0 时只检测首帧">
            <input type="checkbox" :checked="step.timeout !== null" @change="toggleCheckTimeout" />
            检测超时
          </label>
          <CellEditor
            v-if="step.timeout" :cell="step.timeout" type="time" :params="params" :allow-zero-time="true"
            label="检测超时" :error="fieldError('timeout')" @change="(c) => updateCell('timeout', c)"
          />
          <span v-else class="field-hint" title="未配置时引擎默认检测 5s">默认 5s</span>
        </div>
        <div class="field-row">
          <span class="field-label">未命中提示</span>
            <input
            class="cell-input grow" :value="step.throw ?? ''"
            placeholder="可选，默认“模板名 模板不存在”" aria-label="未命中提示"
            @change="setThrow(($event.target as HTMLInputElement).value)"
          />
        </div>
      </template>

      <!-- color -->
      <template v-else-if="step.kind === 'color'">
        <div class="field-row">
          <span class="field-label">坐标</span>
          <CellEditor :cell="step.at" type="coord" :params="params" label="取色坐标" :error="fieldError('at')" @change="(c) => updateCell('at', c)" />
          <span class="field-hint warn">按序首个命中分支获胜；候选可勾选「命中点击」，点击取样点后等待 interval</span>
        </div>
        <div v-for="(exp, ei) in step.expect" :key="ei" class="cand-block" :class="{ 'cell-error': !!fieldError(`expect[${ei}].color`) || !!fieldError('expect') }">
          <div class="field-row">
            <span class="field-label">颜色 {{ ei + 1 }}</span>
            <CellEditor
              :cell="exp.color" type="color" :params="params" :label="`颜色${ei + 1}`"
              :error="fieldError(`expect[${ei}].color`) || fieldError('expect')"
              @change="(c) => updateExpectColor(ei, c)"
            />
            <label class="field-check" title="命中后点击取色坐标的取样点，并等待 interval">
              <input type="checkbox" :checked="exp.click" :aria-label="`命中点击${ei + 1}`" @change="setExpectClick(ei, ($event.target as HTMLInputElement).checked)" />
              命中点击
            </label>
            <button v-if="step.expect.length > 1" type="button" class="mini-btn" title="删除颜色候选" @click.stop="removeExpect(ei)">✕</button>
          </div>
          <BranchContainer
            :model="model" :stack="stack" :container-path="candPath(ei)" :base-path="candBase(ei)"
            :label="candLabel(ei)" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
            :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
            :templates="templates"
            @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
            @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
          />
        </div>
        <div class="field-row">
          <button type="button" class="mini-btn add" title="添加颜色候选" @click.stop="addExpect">+ 添加颜色候选</button>
        </div>
        <BranchContainer
          :model="model" :stack="stack" :container-path="subPath('else')" :base-path="subBase('else')"
          label="颜色未命中" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
          :templates="templates"
          @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
        />
      </template>

      <!-- if -->
      <template v-else-if="step.kind === 'if'">
        <div class="field-row">
          <span class="field-label">条件</span>
          <CellEditor :cell="step.cond" type="bool" :params="params" label="条件" :error="fieldError('cond')" @change="(c) => updateCell('cond', c)" />
          <span class="field-hint">只接受布尔字面量或布尔参数</span>
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
          <span class="field-label">次数</span>
          <input
            class="cell-input num" type="number" min="0"
            :value="step.times" aria-label="循环次数" @change="setTimes(($event.target as HTMLInputElement).value)"
          />
          <span v-if="step.times === 0" class="field-hint warn">0 = 无限循环——请确保体内有 break 或其他退出条件</span>
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

      <!-- call / func -->
      <template v-else-if="step.kind === 'call' || step.kind === 'func'">
        <div class="field-row">
          <span class="field-label">{{ step.kind === 'call' ? '目标脚本' : '目标函数' }}</span>
          <!-- func：文件 + 函数名两级下拉（候选由宿主注入；未注入回退自由输入） -->
          <template v-if="step.kind === 'func' && targetOptions">
            <select
              class="cell-input target-select" :value="fnFileSel" aria-label="函数库文件"
              @change="onFnFileChange(($event.target as HTMLSelectElement).value)"
            >
              <option value="">（选择文件）</option>
              <option v-if="fnFileSel && !fnFiles.some((f) => f.file === fnFileSel)" :value="fnFileSel">{{ fnFileSel }}（已失效）</option>
              <option v-for="f in fnFiles" :key="f.file" :value="f.file">{{ f.file }}</option>
            </select>
            <span class="field-sep">/</span>
            <select
              class="cell-input target-select" :value="fnNameSel" aria-label="函数名" :disabled="!fnFileSel"
              @change="onFnNameChange(($event.target as HTMLSelectElement).value)"
            >
              <option value="">（选择函数）</option>
              <option v-if="fnNameSel && !(curFnFile?.functions ?? []).includes(fnNameSel)" :value="fnNameSel">{{ fnNameSel }}（已失效）</option>
              <option v-for="name in curFnFile?.functions ?? []" :key="name" :value="name">{{ name }}</option>
            </select>
          </template>
          <!-- call：分区脚本下拉 -->
          <select
            v-else-if="step.kind === 'call' && targetOptions"
            class="cell-input target-select" :value="step.target" aria-label="目标脚本"
            @change="applyTarget(($event.target as HTMLSelectElement).value)"
          >
            <option value="">（选择脚本）</option>
            <option v-if="step.target && !callScripts.some((o) => o.target === step.target)" :value="step.target">{{ step.target }}（已失效）</option>
            <option v-for="o in callScripts" :key="o.target" :value="o.target">{{ o.label || o.target }}</option>
          </select>
          <input
            v-else
            class="cell-input" :value="step.target"
            :placeholder="step.kind === 'call' ? 'sub_task.yaml' : 'common/login'"
            :aria-label="step.kind === 'call' ? '目标脚本' : '目标函数'"
            @input="setTarget(($event.target as HTMLInputElement).value)"
          />
          <span v-if="fieldError('target')" class="cell-err-msg">{{ fieldError('target') }}</span>
        </div>
        <div class="field-row col">
          <span class="field-label">参数 args</span>
          <span v-if="targetOptions" class="field-hint">选定目标后按其声明自动生成（默认值已预填，必填项需补齐）；也可手动增删</span>
          <span v-if="fieldError('args')" class="cell-err-msg">{{ fieldError('args') }}</span>
          <div v-for="name in argNames" :key="name" class="arg-row">
            <input
              class="cell-input" :value="name" aria-label="参数名" placeholder="参数名"
              @change="renameArg(name, ($event.target as HTMLInputElement).value)"
            />
            <CellEditor
              :cell="step.args[name]" :type="argType(name)" :params="params"
              :templates="templates"
              :label="`args ${name}`" @change="(c) => updateArgValue(name, c)"
            />
            <button type="button" class="mini-btn" title="删除实参" @click.stop="removeArg(name)">✕</button>
          </div>
          <button type="button" class="mini-btn add" title="添加实参" @click.stop="addArg">+ 添加实参</button>
        </div>
        <template v-if="step.kind === 'func'">
          <BranchContainer
            :model="model" :stack="stack" :container-path="subPath('then')" :base-path="subBase('then')"
            label="成功时" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
            :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
            :templates="templates"
            @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
            @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
          />
          <BranchContainer
            :model="model" :stack="stack" :container-path="subPath('else')" :base-path="subBase('else')"
            label="失败时" :depth="depth + 1" :diagnostics="diagnostics" :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
            :expanded-uuids="expandedUuids" :params="params" :context="context" :resolve-target="resolveTarget"
            :templates="templates"
            @select="(u) => emit('select', u)" @toggle-expand="(u) => emit('toggle-expand', u)"
            @focus="(p) => emit('focus', p)" @add-here="(p, el) => emit('add-here', p, el)"
          />
        </template>
      </template>

      <!-- break -->
      <template v-else-if="step.kind === 'break'">
        <div class="field-hint warn">跳出最近一层 loop；只能放在 loop 子流程内</div>
      </template>

      <!-- throw -->
      <template v-else-if="step.kind === 'throw'">
        <div class="field-row">
          <span class="field-label">原因</span>
          <input
            class="cell-input grow" :value="step.message ?? ''" placeholder="可空"
            aria-label="终止原因" @input="setMessage(($event.target as HTMLInputElement).value)"
          />
          <span class="field-hint warn">会结束整个任务（含调用链）</span>
        </div>
      </template>

      <!-- return -->
      <template v-else-if="step.kind === 'return'">
        <div v-if="context !== 'function'" class="field-hint warn">return 在脚本中非法——只能出现在函数库（functions/）的函数体内</div>
        <div class="field-row">
          <span class="field-label">返回值</span>
          <CellEditor :cell="step.value" type="bool" :params="params" label="返回值" :error="fieldError('value')" @change="(c) => updateCell('value', c)" />
        </div>
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 步骤卡片（plan §8.4 / §9）：19 类全覆盖。
 * - 收起态 = 自然语言摘要（kinds.stepSummary，§9 表文案）；
 * - 展开态 = 该类型强类型控件（无任意键值编辑器）；字段错误按 Diagnostic.field 标红定位；
 * - 左侧动作图标 + 中文名 + 序号 + 上移/下移/复制/删除（全部经 CommandStack）；
 * - find/match/color/if/loop/func 的分支子流程内嵌 BranchContainer（一层内嵌、更深专注）。
 * 纯受控组件：所有写操作构造 Command 提交 stack，自身不改模型。
 */
import { computed, inject, ref, watch, type PropType } from 'vue'
import type { Path } from '../commands'
import { resolveStepList } from '../commands'
import type { Diagnostic } from '../diagnostics'
import { joinStepPath } from '../diagnostics'
import { childContainerPath, containerLabel } from '../selection'
import type { Cell, ParamDecl, Step, StepKind } from '../model'
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
    type: Function as PropType<((kind: 'call' | 'func', target: string) => { params: ParamDecl[] } | null) | undefined>,
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
function candBase(ci: number): string {
  return `${stepPath.value}.candidates[${ci}].steps`
}
function candLabel(ci: number): string {
  return containerLabel(props.step, 'candidates', ci)
}

// ---------- 各类型字段操作 ----------

function toggleRandom(e: Event): void {
  const on = (e.target as HTMLInputElement).checked
  if (on) {
    const cur = s.value.duration
    updateStep({ duration_max: cur && typeof cur.lit === 'string' ? { lit: cur.lit } : { lit: '1s' } })
  } else {
    updateStep({ duration_max: null })
  }
}
function setVerify(v: boolean): void {
  updateStep({ verify: v })
}
function setThrow(v: string): void {
  updateStep({ throw: v.trim() ? v : null })
}
function toggleCheckTimeout(e: Event): void {
  updateStep({ timeout: (e.target as HTMLInputElement).checked ? { lit: '5s' } : null })
}
function toggleFindTimeout(e: Event): void {
  updateStep({ timeout: (e.target as HTMLInputElement).checked ? { lit: '30s' } : null })
}
function toggleMatchTimeout(e: Event): void {
  updateStep({ timeout: (e.target as HTMLInputElement).checked ? { lit: '30s' } : null })
}
function updateBlockAt(i: number, cell: Cell): void {
  updateStep({ block: s.value.block.map((c: Cell, j: number) => (j === i ? cell : c)) })
}
function addBlock(): void {
  updateStep({ block: [...s.value.block, { lit: '' }] })
}
function removeBlock(i: number): void {
  updateStep({ block: s.value.block.filter((_: Cell, j: number) => j !== i) })
}
function addCandidate(): void {
  updateStep({ candidates: [...s.value.candidates, { template: { lit: '' }, click: false, steps: [] }] })
}
function removeCandidate(i: number): void {
  updateStep({ candidates: s.value.candidates.filter((_: unknown, j: number) => j !== i) })
}
function updateCandidateTemplate(i: number, cell: Cell): void {
  updateStep({ candidates: s.value.candidates.map((c: unknown, j: number) => (j === i ? { ...(c as object), template: cell } : c)) })
}
function setCandClick(i: number, on: boolean): void {
  updateStep({ candidates: s.value.candidates.map((c: unknown, j: number) => (j === i ? { ...(c as object), click: on } : c)) })
}
function addExpect(): void {
  updateStep({ expect: [...s.value.expect, { color: { lit: '' }, click: false, steps: [] }] })
}
function removeExpect(i: number): void {
  updateStep({ expect: s.value.expect.filter((_: unknown, j: number) => j !== i) })
}
function updateExpectColor(i: number, cell: Cell): void {
  updateStep({ expect: s.value.expect.map((c: unknown, j: number) => (j === i ? { ...(c as object), color: cell } : c)) })
}
function setExpectClick(i: number, on: boolean): void {
  updateStep({ expect: s.value.expect.map((c: unknown, j: number) => (j === i ? { ...(c as object), click: on } : c)) })
}
function setTimes(raw: string): void {
  const n = Number(raw)
  updateStep({ times: Number.isFinite(n) && n >= 0 ? Math.floor(n) : 0 })
}
function setTarget(v: string): void {
  updateStep({ target: v })
}
function setMessage(v: string): void {
  updateStep({ message: v === '' ? null : v })
}

// ---------- call/func 目标下拉（宿主经 provide(SE_TARGET_OPTIONS) 注入候选与解析） ----------

const targetOptions = inject<SeTargetOptions | null>(SE_TARGET_OPTIONS, null)
const callScripts = computed(() => targetOptions?.callScripts ?? [])
const fnFiles = computed(() => targetOptions?.funcFiles ?? [])

/** func 两级下拉的本地选择态：target 外部变化（撤销/加载/跳转）时从 target 回同步。 */
const fnFileSel = ref('')
const fnNameSel = ref('')
const curFnFile = computed(() => fnFiles.value.find((f) => f.file === fnFileSel.value) ?? null)

function splitFnTarget(t: string): [string, string] {
  const s = String(t || '')
  const i = s.indexOf('/')
  return i >= 0 ? [s.slice(0, i), s.slice(i + 1)] : [s, '']
}
watch(
  () => String(s.value.target ?? ''),
  (t) => {
    const [file, fn] = splitFnTarget(t)
    fnFileSel.value = file
    fnNameSel.value = fn
  },
  { immediate: true },
)

function onFnFileChange(file: string): void {
  fnFileSel.value = file
  fnNameSel.value = '' // 换文件后原函数名视为失效，选中函数名才统一下发 target
}
function onFnNameChange(fn: string): void {
  fnNameSel.value = fn
  if (fnFileSel.value && fn) void applyTarget(`${fnFileSel.value}/${fn}`)
}

/**
 * 下发目标；宿主注入了解析器时一并按目标声明重生成 args（默认值预填、必填补类型空值），
 * 单条 update_step = 一次撤销。await 期间目标若又被改动则放弃（由最新一次变更接管）。
 */
async function applyTarget(next: string): Promise<void> {
  if (!next) {
    updateStep({ target: '', args: {} })
    return
  }
  if (!targetOptions) {
    updateStep({ target: next })
    return
  }
  const prev = String(s.value.target ?? '')
  let decls: ParamDecl[] | null = null
  try {
    decls = await targetOptions.resolveParams(props.step.kind === 'call' ? 'call' : 'func', next)
  } catch {
    decls = null // 解析失败不阻塞改目标：args 保持原样（校验层兜底）
  }
  if (String(s.value.target ?? '') !== prev) return
  try {
    updateStep(decls ? { target: next, args: argsFromDecls(decls) } : { target: next })
  } catch {
    // await 期间步骤已被删除（resolveStep 抛错）——放弃本次下发
  }
}

/** 按目标声明生成完整 args：有默认值填默认值；必填填类型空值待用户补。 */
function argsFromDecls(decls: ParamDecl[]): Record<string, Cell> {
  const out: Record<string, Cell> = {}
  for (const d of decls) {
    out[d.name] = { lit: d.default !== null && d.default !== undefined ? d.default : emptyLitFor(d.type) }
  }
  return out
}
function emptyLitFor(type: ParamDecl['type']): unknown {
  switch (type) {
    case 'bool':
      return false
    case 'coord':
      return [0.5, 0.5]
    default:
      return ''
  }
}

// ---------- args（call/func） ----------

const argNames = computed<string[]>(() => Object.keys(s.value.args ?? {}))

function argType(name: string): ParamDecl['type'] {
  const kind = props.step.kind === 'call' ? 'call' : 'func'
  const target = String(s.value.target ?? '')
  // 宿主注入的同步缓存优先（call 实参类型需异步拉取后才有）；prop 解析器兜底
  const decls = targetOptions?.resolveParamsSync
    ? targetOptions.resolveParamsSync(kind, target)
    : props.resolveTarget?.(kind, target)?.params
  return decls?.find((d) => d.name === name)?.type ?? 'text'
}
function updateArgValue(name: string, cell: Cell): void {
  updateStep({ args: { ...s.value.args, [name]: cell } })
}
function removeArg(name: string): void {
  const next = { ...s.value.args }
  delete next[name]
  updateStep({ args: next })
}
function renameArg(oldName: string, raw: string): void {
  const name = raw.trim()
  if (!name || name === oldName) return
  if (name in s.value.args) return // 重复键直接忽略（校验层另有 args.unknown 提示）
  const next: Record<string, Cell> = {}
  for (const [k, v] of Object.entries(s.value.args)) next[k === oldName ? name : k] = v as Cell
  updateStep({ args: next })
}
function addArg(): void {
  const args = s.value.args ?? {}
  let i = 1
  while (`param${i}` in args) i++
  updateStep({ args: { ...args, [`param${i}`]: { lit: '' } } })
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
</style>
