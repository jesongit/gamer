"""Explicit real-task acceptance against a named, scheduler-disabled clone.

Unlike the synthetic smoke test, this consumes the existing four account tasks.
Require --run-existing-tasks so it cannot be mistaken for an offline unit test.
"""
import argparse
import json
import pathlib
import subprocess


CHECK = r"""import hashlib,io,json,pathlib,time,tomllib,urllib.error,urllib.request,zipfile
root=pathlib.Path('/data');config=tomllib.loads((root/'config.toml').read_text())
assert config['scheduler_enabled'] is False,'Acceptance requires automatic scheduling off'
assert config['trace']['production'] is True
assert config['trace']['capture_interval_ms']==10000
assert config['trace']['run_max_bytes']==67108864
assert config['trace']['global_max_bytes']==536870912
token=(root/'state/local-admin-token').read_text().strip()
def request(path,method='GET',value=None,authenticated=True):
 data=json.dumps(value).encode() if value is not None else None
 headers={'Content-Type':'application/json'}
 if authenticated:headers['X-Admin-Token']=token
 return urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:8443'+path,data=data,headers=headers,method=method),timeout=120)
def api(path,method='GET',value=None):
 with request(path,method,value) as response:return json.load(response)
def expected_error(path,code):
 try:request(path,authenticated=False);raise AssertionError('Anonymous request accepted')
 except urllib.error.HTTPError as error:assert error.code==code,error.code
extensions=api('/api/extensions')['extensions']
yaml=next(x for x in extensions if x['id']=='gamer-yaml')
assert yaml['version']=='0.3.23-local.20261018' and yaml['state']=='running',yaml
assert any(x['id']=='gamer-ai' and x['state']=='enabled' for x in extensions)
tasks=api('/api/tasks');assert len(tasks)==4 and all(x['enabled'] for x in tasks)
results=[]
for task in tasks:
 device=task['app']['device_id']
 assert api('/api/devices/'+device+'/run')['active'] is False
 dispatched=api('/api/tasks/'+task['id']+'/run','POST',{})
 identity=dispatched['run_id'];start=time.monotonic()
 print(json.dumps({'event':'started','task_id':task['id'],'run_id':identity}),flush=True)
 while True:
  record=api('/api/runs/'+identity)
  if record['state'] in ('success','failed','cancelled'):break
  if time.monotonic()-start>1200:raise TimeoutError('Task did not terminate in 20 minutes: '+identity)
  time.sleep(3)
 assert record['task_id']==task['id'] and record['device_id']==device
 assert record['state']=='success',{'run_id':identity,'state':record['state'],'error':record.get('error')}
 assert record['_diagnostics_version']==1
 trace=api('/api/runs/'+identity+'/trace?limit=100')
 assert trace['capture_policy']=='production' and trace['policy_version']==1
 assert len(trace['images'])<=5 and not trace['has_more']
 assert trace['identity'],'Frozen source and build identity missing'
 expected_error('/api/runs/'+identity+'/diagnostics',401)
 with request('/api/runs/'+identity+'/diagnostics') as response:
  assert response.headers['Cache-Control']=='private, no-store'
  raw=response.read(160*1024*1024)
 with zipfile.ZipFile(io.BytesIO(raw)) as archive:
  assert archive.testzip() is None
  manifest=json.loads(archive.read('manifest.json'))
  assert manifest['run_id']==identity
  exported=json.loads(archive.read('run.json'))
  assert exported['run_id']==identity
  assert {'run.json','events.jsonl','trace.json','manifest.json'}.issubset(archive.namelist())
  assert not any('browser-profiles' in n or n.endswith('credentials.key') for n in archive.namelist())
  image_count=sum(n.startswith('assets/') for n in archive.namelist())
 for _ in range(10):
  if api('/api/devices/'+device+'/run')['active'] is False:break
  time.sleep(1)
 assert api('/api/devices/'+device+'/run')['active'] is False
 api('/api/browser-targets/'+device+'/disconnect','POST',{})
 result={'task_id':task['id'],'run_id':identity,'state':record['state'],
         'seconds':round(time.monotonic()-start,1),'images':len(trace['images']),
         'sampled_out':trace['sampled_out'],'compacted_images':trace['compacted_images'],
         'trace_bytes':trace['bytes'],'archive_bytes':len(raw),'archive_sha256':hashlib.sha256(raw).hexdigest(),
         'archive_assets':image_count,'identity_present':bool(trace['identity'])}
 results.append(result);print(json.dumps({'event':'completed',**result}),flush=True)
print(json.dumps({'event':'receipt','real_account_tasks':results,'automatic_scheduler':False,'all_success':True}),flush=True)
"""


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--container',required=True)
    parser.add_argument('--receipt',type=pathlib.Path,required=True)
    parser.add_argument('--run-existing-tasks',action='store_true',required=True)
    args=parser.parse_args()
    assert args.container.startswith('gamer-diagnostics-acceptance-'), 'Only the explicit acceptance clone is allowed'
    process=subprocess.Popen(['docker','exec','-i',args.container,'python3','-u','-c',CHECK],
                             stdout=subprocess.PIPE,text=True,encoding='utf-8')
    receipt=None
    for line in process.stdout:
        print(line.rstrip(),flush=True)
        data=json.loads(line)
        if data.get('event')=='receipt':receipt=data
    assert process.wait()==0 and receipt, 'Acceptance failed; inspect the exact run_id'
    args.receipt.write_text(json.dumps(receipt,indent=2),encoding='utf-8')


if __name__=='__main__':
    main()
