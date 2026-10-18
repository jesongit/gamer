"""Known-page fault/cancellation acceptance on the explicitly named test clone."""
import argparse
import json
import pathlib
import subprocess

CHECK=r"""import hashlib,http.server,io,json,pathlib,struct,threading,time,urllib.error,urllib.request,zipfile,zlib
token=pathlib.Path('/data/state/local-admin-token').read_text().strip()
def request(path,method='GET',value=None,content_type='application/json'):
 data=value if isinstance(value,bytes) else json.dumps(value).encode() if value is not None else None
 req=urllib.request.Request('http://127.0.0.1:8443'+path,data=data,headers={'X-Admin-Token':token,'Content-Type':content_type},method=method)
 try:return urllib.request.urlopen(req,timeout=120)
 except urllib.error.HTTPError as error:
  if not (path.endswith('/diagnostics') and error.code==409):print('Fixture HTTP error: '+error.read(3000).decode(),flush=True)
  raise
def api(path,method='GET',value=None,content_type='application/json'):
 with request(path,method,value,content_type) as response:
  return None if response.status==204 else json.load(response)
class Fixture(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  body=b'<html style="background:#18324c;color:white"><h1>Synthetic diagnostic fault fixture</h1><p>No game or account data.</p></html>'
  self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
 def log_message(self,*args):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',18991),Fixture)
threading.Thread(target=server.serve_forever,daemon=True).start()
pkg='diagnostics-acceptance-20261018';device='browser-diagnostics-acceptance-20261018'
assert not any(x['id']==pkg for x in api('/api/packages')['packages'])
assert not any(x['id']==device for x in api('/api/browser-targets'))
api('/api/packages','POST',{'id':pkg,'name':'Synthetic diagnostics acceptance','targets':{'web':{'url_prefixes':['http://127.0.0.1:18991/']}}})
api('/api/browser-targets','POST',{'id':device,'name':'Synthetic diagnostics acceptance','profile_id':device,'url':'http://127.0.0.1:18991/','width':640,'height':360})
def png():
 raw=b''.join(b'\0'+bytes(((x//3+y//3)%2)*230+10 for x in range(32) for _ in range(3)) for y in range(32))
 def chunk(k,v):return struct.pack('>I',len(v))+k+v+struct.pack('>I',zlib.crc32(k+v)&0xffffffff)
 return b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',32,32,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(raw))+chunk(b'IEND',b'')
prefix='/api/packages/'+pkg+'/plugins/gamer-yaml/resources/'
api(prefix+'templates/absent.png','PUT',png(),'image/png')
for name,timeout in [('failure',12),('cancel',60)]:
 source='name: Synthetic '+name+'\nrun:\n  - wait: {template: absent.png, threshold: 0.99}\n    timeout: '+str(timeout)+'s\n'
 api(prefix+'automations/'+name+'.yaml','PUT',{'content':source})
results=[]
for name,expected in [('failure','failed'),('cancel','cancelled')]:
 identity=api('/api/runs','POST',{'runner_id':'gamer-yaml','entrypoint':pkg+'/'+name+'.yaml','device_id':device,'content_package':pkg})['run_id']
 start=time.monotonic();print(json.dumps({'event':'started','case':name,'run_id':identity}),flush=True)
 try:
  request('/api/runs/'+identity+'/diagnostics');raise AssertionError('Active diagnostics accepted')
 except urllib.error.HTTPError as error:assert error.code==409,error.code
 if name=='cancel':
  time.sleep(3);api('/api/runs/'+identity+'/cancel','POST',{})
 while True:
  record=api('/api/runs/'+identity)
  if record['state'] in ('failed','success','cancelled'):break
  assert time.monotonic()-start<90
  time.sleep(1)
 assert record['state']==expected,record
 trace=api('/api/runs/'+identity+'/trace?limit=100')
 kinds=[x['kind'] for x in trace['images']]
 if name=='failure':
  assert 'error_last_consumed' in kinds and 'error_fresh' in kinds,kinds
 assert trace['sampled_out']>0,trace
 with request('/api/runs/'+identity+'/diagnostics') as response:raw=response.read(20*1024*1024)
 with zipfile.ZipFile(io.BytesIO(raw)) as archive:
  assert archive.testzip() is None
  manifest=json.loads(archive.read('manifest.json'));assert manifest['run_id']==identity
  frozen=json.loads(archive.read('trace.json'))
 results.append({'case':name,'run_id':identity,'state':expected,'image_kinds':kinds,
                 'sampled_out':trace['sampled_out'],'images':len(trace['images']),
                 'archive_bytes':len(raw),'archive_sha256':hashlib.sha256(raw).hexdigest()})
 print(json.dumps({'event':'completed',**results[-1]}),flush=True)
 for _ in range(20):
  if not api('/api/devices/'+device+'/run')['active']:break
  time.sleep(1)
 assert not api('/api/devices/'+device+'/run')['active']
api('/api/browser-targets/'+device+'/disconnect','POST',{})
api('/api/browser-targets/'+device,'DELETE')
api('/api/packages/'+pkg,'DELETE')
server.shutdown()
print(json.dumps({'event':'receipt','fault_cases':results,'own_fixture_targets_removed':True}),flush=True)
"""

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--container',required=True)
    p.add_argument('--receipt',type=pathlib.Path,required=True)
    a=p.parse_args()
    assert a.container.startswith('gamer-diagnostics-acceptance-')
    process=subprocess.Popen(['docker','exec',a.container,'python3','-u','-c',CHECK],stdout=subprocess.PIPE,text=True,encoding='utf-8')
    receipt=None
    for line in process.stdout:
        print(line.rstrip(),flush=True)
        try:data=json.loads(line)
        except json.JSONDecodeError:continue
        if data.get('event')=='receipt':receipt=data
    assert process.wait()==0 and receipt,'Fault test failed; preserve the exact run for diagnosis'
    a.receipt.write_text(json.dumps(receipt,indent=2),encoding='utf-8')

if __name__=='__main__':main()
