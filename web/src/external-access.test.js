// @vitest-environment happy-dom
import { mount, flushPromises } from '@vue/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'
import ExternalAccessSettings from './components/ExternalAccessSettings.vue'
import { api } from './api'
vi.mock('./api', () => ({ api: Object.fromEntries(['listMcpTokens','listExternalControl','listDevices','listPackages','listExtensions','listRunners','getMcpCatalog','createMcpToken','revokeMcpToken','startExternalControl','externalControlAction'].map(name => [name,vi.fn()])) }))
const wrappers=[]
afterEach(() => { wrappers.splice(0).forEach(w=>w.unmount());vi.clearAllMocks() })
function setup() {
  api.listMcpTokens.mockResolvedValue({tokens:[{token_id:'one',label:'Selected Agent',expires_at:Math.floor(Date.now()/1000)+3600,revoked:false,scopes:['control','observe'],allowed:{devices:['device-one'],packages:['default'],plugins:[],runners:[],runs:[],tasks:[],media:[]}}]})
  api.listExternalControl.mockResolvedValue({sessions:[]})
  api.listDevices.mockResolvedValue([{id:'device-one',name:'Allowed target'},{id:'device-two',name:'Other target'}])
  api.listPackages.mockResolvedValue({packages:[{id:'default',name:'Default'}]})
  api.listExtensions.mockResolvedValue({extensions:[]});api.listRunners.mockResolvedValue([])
  api.getMcpCatalog.mockResolvedValue({endpoint:'http://127.0.0.1:8443/api/mcp',tools:[]})
  const wrapper=mount(ExternalAccessSettings);wrappers.push(wrapper);return wrapper
}
describe('external Agent management',()=>{
  it('loads only metadata until the user selects an authorized target and starts control',async()=>{
    const w=setup();await flushPromises()
    expect(api.startExternalControl).not.toHaveBeenCalled()
    expect(api.createMcpToken).not.toHaveBeenCalled()
    const start=w.findAll('button').find(b=>b.text()==='开始外部控制')
    expect(start.attributes('disabled')).toBeDefined()
    const selectors=w.findAll('select')
    await selectors[0].setValue('one');await flushPromises()
    expect(selectors[1].text()).toContain('Allowed target')
    expect(selectors[1].text()).not.toContain('Other target')
    await selectors[1].setValue('device-one')
    api.startExternalControl.mockResolvedValue({session:{state:'starting'}})
    await w.get('.control-form').trigger('submit');await flushPromises()
    expect(api.startExternalControl).toHaveBeenCalledExactlyOnceWith({token_id:'one',device_id:'device-one',content_package:null,ttl_seconds:300})
  })
  it('creates a token with explicit empty object ranges and hides its secret on request',async()=>{
    const w=setup();await flushPromises()
    await w.findAll('button').find(b=>b.text()==='创建令牌').trigger('click')
    await w.get('input[placeholder="例如：我的自动化 Agent"]').setValue('Read only Agent')
    api.createMcpToken.mockResolvedValue({token:'one-time-secret',metadata:{token_id:'new'}})
    await w.get('.token-form').trigger('submit');await flushPromises()
    expect(api.createMcpToken).toHaveBeenCalledExactlyOnceWith({label:'Read only Agent',scopes:['observe'],allowed:{devices:[],packages:[],plugins:[],runners:[],runs:[],tasks:[],media:[]},expires_in_seconds:86400})
    expect(w.get('[aria-label="新建 MCP 令牌"]').element.value).toBe('one-time-secret')
    await w.findAll('button').find(b=>b.text()==='我已保存，隐藏令牌').trigger('click')
    expect(w.find('[aria-label="新建 MCP 令牌"]').exists()).toBe(false)
    expect(api.startExternalControl).not.toHaveBeenCalled()
  })
})
