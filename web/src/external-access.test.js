// @vitest-environment happy-dom
import { mount, flushPromises } from '@vue/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'
const { decision } = vi.hoisted(() => ({ decision: vi.fn(async () => true) }))
vi.mock('./components/ui/useConfirmDialog', () => ({ useConfirmDialog: () => decision }))
import ExternalAccessSettings from './components/ExternalAccessSettings.vue'
import { api } from './api'
vi.mock('./api', () => ({ api: Object.fromEntries(['listMcpTokens','getMcpCatalog','createMcpToken','viewMcpToken','revokeMcpToken','getSystemSettings','saveSystemSettings'].map(name => [name,vi.fn()])) }))
const wrappers=[]
afterEach(() => { wrappers.splice(0).forEach(w=>w.unmount());vi.restoreAllMocks();vi.resetAllMocks() })
const token={token_id:'one',label:'我的 Agent',expires_at:0,revoked:false,can_view:true}
function setup(tokens=[token], failRead=false) {
  decision.mockResolvedValue(true)
  api.listMcpTokens.mockResolvedValue({tokens})
  if (failRead) api.listMcpTokens.mockRejectedValueOnce(new Error('连接中断'))
  api.getMcpCatalog.mockResolvedValue({endpoint:'http://127.0.0.1:8443/api/mcp',tools:[]})
  api.getSystemSettings.mockResolvedValue({saved:{mcp_allow_remote:false,idle_power_secs:120},active:{mcp_allow_remote:false},revision:'r1'})
  const wrapper=mount(ExternalAccessSettings);wrappers.push(wrapper);return wrapper
}
function button(w,text) { return w.findAll('button').find(b=>b.text()===text) }

describe('simple MCP access settings',()=>{
  it('shows connection and expiry without permission selectors or control forms',async()=>{
    const w=setup();await flushPromises()
    expect(w.text()).toContain('http://127.0.0.1:8443/api/mcp')
    expect(w.text()).toContain('不过期')
    expect(w.find('select').exists()).toBe(false)
    expect(w.get('input[type="checkbox"]').element.checked).toBe(false)
    expect(w.find('.control-form').exists()).toBe(false)
    expect(api.viewMcpToken).not.toHaveBeenCalled()
  })
  it('creates a permanent token with only name and lifetime',async()=>{
    const w=setup([]);await flushPromises()
    api.createMcpToken.mockResolvedValue({token:'synthetic-secret',metadata:{token_id:'new'}})
    api.listMcpTokens.mockResolvedValue({tokens:[{...token,token_id:'new'}]})
    await w.find('input:not([type])').setValue('我的 Agent')
    await w.get('.token-form').trigger('submit');await flushPromises()
    expect(api.createMcpToken).toHaveBeenCalledExactlyOnceWith({label:'我的 Agent',expires_in_seconds:0})
    expect(w.get('.token-secret').element.value).toBe('synthetic-secret')
  })
  it('converts finite hours and rejects negative lifetime',async()=>{
    const w=setup([]);await flushPromises()
    api.createMcpToken.mockResolvedValue({token:'synthetic',metadata:{token_id:'new'}})
    await w.get('input[type="number"]').setValue(48)
    await w.get('form').trigger('submit');await flushPromises()
    expect(api.createMcpToken).toHaveBeenCalledWith({label:'外部 Agent',expires_in_seconds:172800})
    await w.get('input[type="number"]').setValue(-1)
    await w.get('form').trigger('submit');await flushPromises()
    expect(api.createMcpToken).toHaveBeenCalledTimes(1)
    expect(w.get('[role="alert"]').text()).toContain('0～8784')
  })
  it('views an existing secret on demand and hides it again',async()=>{
    const w=setup();await flushPromises()
    api.viewMcpToken.mockResolvedValue({token:'returned-secret'})
    await button(w,'查看').trigger('click');await flushPromises()
    expect(api.viewMcpToken).toHaveBeenCalledExactlyOnceWith('one')
    expect(w.get('.token-secret').element.value).toBe('returned-secret')
    await button(w,'隐藏').trigger('click');await flushPromises()
    expect(w.find('.token-secret').exists()).toBe(false)
  })
  it('deletes the record after success and keeps it when deletion fails',async()=>{
    const w=setup();await flushPromises()
    api.revokeMcpToken.mockRejectedValueOnce(new Error('写入失败'))
    await button(w,'删除').trigger('click');await flushPromises()
    expect(w.findAll('.tokens>li')).toHaveLength(1)
    expect(w.text()).toContain('写入失败')
    api.revokeMcpToken.mockResolvedValue({deleted:true})
    await button(w,'删除').trigger('click');await flushPromises()
    expect(api.revokeMcpToken).toHaveBeenCalledWith('one')
    expect(w.findAll('.tokens>li')).toHaveLength(0)
  })
  it('preserves an old hash-only credential with a clear unavailable-view state',async()=>{
    const w=setup([{...token,can_view:false}]);await flushPromises()
    expect(button(w,'查看').attributes('disabled')).toBeDefined()
    expect(button(w,'删除').attributes('disabled')).toBeUndefined()
    expect(w.text()).toContain('旧令牌无法查看原文')
  })
  it('retries a failed initial read',async()=>{
    const w=setup([token],true);await flushPromises()
    expect(w.text()).toContain('连接中断')
    await button(w,'重试读取').trigger('click');await flushPromises()
    expect(w.find('[role="alert"]').exists()).toBe(false)
  })
  it('enables remote access using fresh settings and preserves the public scheme and port',async()=>{
    const w=setup();await flushPromises()
    vi.spyOn(window.location, 'href', 'get').mockReturnValue('https://gamer.example:18443/#/console')
    api.getSystemSettings.mockResolvedValue({saved:{mcp_allow_remote:false,idle_power_secs:90},revision:'r2'})
    api.saveSystemSettings.mockResolvedValue({active:{mcp_allow_remote:true}})
    await w.get('input[type="checkbox"]').setValue(true);await flushPromises()
    expect(api.saveSystemSettings).toHaveBeenCalledExactlyOnceWith({mcp_allow_remote:true,idle_power_secs:90},'r2')
    expect(w.text()).toContain('https://gamer.example:18443/api/mcp')
    expect(w.get('input[type="checkbox"]').element.checked).toBe(true)
    expect(w.text()).toContain('已允许远程 MCP 连接')
  })
  it('keeps remote access disabled when the save fails and allows retry',async()=>{
    const w=setup();await flushPromises()
    api.saveSystemSettings.mockRejectedValueOnce(new Error('配置已发生变化，请重新读取后再保存'))
    await w.get('input[type="checkbox"]').setValue(true);await flushPromises()
    expect(w.get('input[type="checkbox"]').element.checked).toBe(false)
    expect(w.get('[role="alert"]').text()).toContain('配置已发生变化')
    api.saveSystemSettings.mockResolvedValue({active:{mcp_allow_remote:true}})
    await w.get('input[type="checkbox"]').setValue(true);await flushPromises()
    expect(w.get('input[type="checkbox"]').element.checked).toBe(true)
  })
  it('shows the listener restriction even if the remote setting is enabled',async()=>{
    const w=setup();await flushPromises()
    api.getMcpCatalog.mockResolvedValue({endpoint:'http://127.0.0.1:8443/api/mcp',local_only:true})
    api.getSystemSettings.mockResolvedValue({active:{mcp_allow_remote:true}})
    await button(w,'刷新').trigger('click');await flushPromises()
    expect(w.text()).toContain('关闭 local_only 并重启服务')
    expect(w.text()).toContain('http://127.0.0.1:8443/api/mcp')
  })
})
