"""Four concurrent known-page failures through the real YAML/browser pipeline."""
import argparse
import json
import pathlib
import subprocess

from test_gamer_diagnostics_fault import CHECK as FAULT_CHECK

# Reuse only the isolated package/template/server fixture; no test result is reused.
CHECK=FAULT_CHECK.split('results=[]\n',1)[0]+r"""
import concurrent.futures
devices=[device]
for index in range(1,4):
 target=device+'-parallel-'+str(index)
 assert not any(x['id']==target for x in api('/api/browser-targets'))
 api('/api/browser-targets','POST',{'id':target,'name':'Synthetic parallel '+str(index),'profile_id':target,'url':'http://127.0.0.1:18991/','width':640,'height':360})
 devices.append(target)
def submit(target):
 return api('/api/runs','POST',{'runner_id':'gamer-yaml','entrypoint':pkg+'/failure.yaml','device_id':target,'content_package':pkg})['run_id']
with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:identities=list(executor.map(submit,devices))
print(json.dumps({'event':'started','run_ids':identities}),flush=True)
try:submit(devices[0]);raise AssertionError('Duplicate run was accepted on a busy device')
except urllib.error.HTTPError as error:assert error.code==409
latencies=[];start=time.monotonic()
while True:
 before=time.monotonic()
 with urllib.request.urlopen('http://127.0.0.1:8443/health/ready',timeout=5) as response:assert response.status==200
 latencies.append((time.monotonic()-before)*1000)
 records=[api('/api/runs/'+identity) for identity in identities]
 if all(x['state'] in ('failed','success','cancelled') for x in records):break
 assert time.monotonic()-start<90
 time.sleep(.2)
assert all(x['state']=='failed' for x in records)
results=[]
for identity,target in zip(identities,devices):
 trace=api('/api/runs/'+identity+'/trace?limit=100');kinds=[x['kind'] for x in trace['images']]
 assert 'error_last_consumed' in kinds and 'error_fresh' in kinds,(identity,kinds)
 for _ in range(20):
  if not api('/api/devices/'+target+'/run')['active']:break
  time.sleep(.2)
 assert not api('/api/devices/'+target+'/run')['active']
 api('/api/browser-targets/'+target+'/disconnect','POST',{})
 api('/api/browser-targets/'+target,'DELETE')
 results.append({'run_id':identity,'state':'failed','error_frames_preserved':True,'sampled_out':trace['sampled_out']})
api('/api/packages/'+pkg,'DELETE');server.shutdown()
assert max(latencies)<5000
print(json.dumps({'event':'receipt','parallel_runs':4,'cases':results,'health_samples':len(latencies),
                  'health_max_ms':round(max(latencies),1),'duplicate_run_rejected':True,
                  'all_slots_released':True,'own_fixture_targets_removed':True}),flush=True)
"""

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--container',required=True)
    p.add_argument('--receipt',type=pathlib.Path,required=True)
    a=p.parse_args();assert a.container.startswith('gamer-diagnostics-acceptance-')
    process=subprocess.Popen(['docker','exec',a.container,'python3','-u','-c',CHECK],stdout=subprocess.PIPE,text=True,encoding='utf-8')
    receipt=None
    for line in process.stdout:
        print(line.rstrip(),flush=True)
        try:data=json.loads(line)
        except json.JSONDecodeError:continue
        if data.get('event')=='receipt':receipt=data
    assert process.wait()==0 and receipt,'Parallel diagnostic acceptance failed'
    a.receipt.write_text(json.dumps(receipt,indent=2),encoding='utf-8')

if __name__=='__main__':main()
