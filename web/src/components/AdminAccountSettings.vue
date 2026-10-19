<template>
  <section class="card account-card" aria-label="管理员账号">
    <div class="account-head"><b>管理员账号</b><button class="btn btn-sm" :disabled="busy" @click="load">重新读取</button></div>
    <p class="hint">新密码留空时只修改用户名。保存后所有管理登录退出，后台任务继续运行。</p>
    <p v-if="loading" role="status">正在读取账号…</p>
    <form v-if="view" @submit.prevent="save">
      <fieldset :disabled="busy">
        <div class="account-fields">
          <label>用户名<input v-model="username" class="input" autocomplete="username" required maxlength="64" pattern="[A-Za-z0-9._\-]+" aria-describedby="username-help" /><small id="username-help">1～64 个英文字母、数字、点、下划线或短横线，区分大小写。</small></label>
          <label>当前密码<input v-model="currentPassword" class="input" type="password" autocomplete="current-password" required maxlength="1024" /></label>
          <label>新密码（可选）<input v-model="newPassword" class="input" type="password" autocomplete="new-password" :disabled="!view.password_editable" minlength="8" maxlength="1024" /></label>
          <label>确认新密码<input v-model="confirmPassword" class="input" type="password" autocomplete="new-password" :disabled="!view.password_editable" maxlength="1024" /></label>
        </div>
        <p v-if="!view.password_editable" class="hint" role="status">密码由 GAMER_ADMIN_PASSWORD 环境变量管理，请修改该变量并重启服务。这里仍可修改用户名。</p>
        <p v-else class="hint">新密码至少 8 位。此处修改即时生效，并在重启后保留。</p>
        <button class="btn btn-primary btn-sm" type="submit" :disabled="!dirty || !currentPassword">{{ saving ? '保存中…' : '保存并重新登录' }}</button>
      </fieldset>
    </form>
    <p v-if="error" class="error" role="alert">{{ error }}</p>
  </section>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
import { accountChanged } from '../auth'
const view = ref(null), username = ref(''), currentPassword = ref(''), newPassword = ref(''), confirmPassword = ref('')
const loading = ref(false), saving = ref(false), error = ref('')
const busy = computed(() => loading.value || saving.value)
const dirty = computed(() => !!view.value && (username.value !== view.value.username || !!newPassword.value))
const errors = {
  invalid_username: '用户名须为 1～64 个英文字母、数字、点、下划线或短横线。',
  invalid_current_password: '当前密码不正确，请重新输入。',
  current_password_required: '请输入当前密码。',
  weak_password: '新密码须为 8～1024 字节。', password_mismatch: '两次新密码不一致。',
  account_revision_conflict: '账号已被其他页面修改，请重新读取。',
  account_config_changed: '配置文件中的账号已变化，请重启服务后重新读取。',
  account_config_invalid: '配置文件无法解析，请检查配置文件。',
  account_config_read_failed: '无法读取配置文件，请检查路径和权限。',
  account_config_write_failed: '无法保存配置文件，请检查文件权限。账号尚未修改。',
  password_managed_by_environment: '密码由环境变量管理，请修改该变量并重启服务。',
  too_many_attempts: '密码验证失败次数过多，请稍后再试。',
  account_update_busy: '账号正在保存，请稍后再试。', account_unchanged: '账号没有变化。',
}
async function load() {
  if (busy.value) return
  loading.value = true; error.value = ''
  try {
    view.value = await api.getAdminAccount()
    username.value = view.value.username
    currentPassword.value = newPassword.value = confirmPassword.value = ''
  } catch (e) { error.value = e.message } finally { loading.value = false }
}
async function save() {
  if (busy.value || !dirty.value) return
  error.value = ''
  if (newPassword.value !== confirmPassword.value) { error.value = errors.password_mismatch; return }
  saving.value = true
  try {
    await api.updateAdminAccount({ username: username.value, current_password: currentPassword.value,
      new_password: newPassword.value, confirm_password: confirmPassword.value, expected_revision: view.value.revision })
    currentPassword.value = newPassword.value = confirmPassword.value = ''
    accountChanged(username.value)
  } catch (e) {
    error.value = errors[e.code] || e.message
    // Do not keep a rejected current password in the form.
    currentPassword.value = ''
  } finally { saving.value = false }
}
onMounted(load)
</script>

<style scoped>
.account-card{padding:16px;margin-bottom:16px}.account-head{display:flex;align-items:center;justify-content:space-between;gap:12px}.hint{font-size:12px;color:var(--text-2);line-height:1.7}.account-fields{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:16px}.account-fields label{display:flex;flex-direction:column;gap:6px;font-size:13px;min-width:0}.account-fields small{color:var(--text-2);font-size:12px;line-height:1.6}.account-fields .input{width:100%;box-sizing:border-box}.error{font-size:13px;color:var(--danger);line-height:1.7}fieldset{border:0;margin:0;padding:0;min-width:0}
@media(max-width:600px){.account-fields{grid-template-columns:minmax(0,1fr)}}
</style>
