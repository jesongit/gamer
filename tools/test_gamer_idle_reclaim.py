"""Measure the real 30-minute browser idle timer with a local synthetic page."""
import argparse
import json
import pathlib
import subprocess

CHECK=r"""import http.server,json,pathlib,threading,time,tomllib,urllib.request
root=pathlib.Path('/data');config=tomllib.loads((root/'config.toml').read_text());assert config['browser_idle_secs']==1800
token=(root/'state/local-admin-token').read_text().strip();identity='browser-idle-acceptance-20261018'
def api(path,method='GET',data=None):
 body=json.dumps(data).encode() if data is not None else None
 req=urllib.request.Request('http://127.0.0.1:8443'+path,data=body,headers={'X-Admin-Token':token,'Content-Type':'application/json'},method=method)
 with urllib.request.urlopen(req,timeout=30) as r:return None if r.status==204 else json.load(r)
class Fixture(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  self.send_response(200);self.send_header('Content-Type','text/html');self.end_headers();self.wfile.write(b'<html><title>Idle reclaim acceptance</title><p>Local synthetic page</p></html>')
 def log_message(self,*args):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',18993),Fixture);threading.Thread(target=server.serve_forever,daemon=True).start()
assert not any(x['id']==identity for x in api('/api/browser-targets'))
api('/api/browser-targets','POST',{'id':identity,'name':'Local idle reclaim acceptance','profile_id':identity,'url':'http://127.0.0.1:18993/','width':640,'height':360})
api('/api/browser-targets/'+identity+'/connect','POST',{})
start=time.monotonic();start_time=time.time();samples=[]
while True:
 target=next(x for x in api('/api/browser-targets') if x['id']==identity)
 elapsed=time.monotonic()-start
 if target['status']=='offline':break
 assert elapsed<1860,'Browser did not close within 60 seconds of the configured idle timeout'
 with urllib.request.urlopen('http://127.0.0.1:8443/health/ready',timeout=30) as response:assert response.status==200
 samples.append({'elapsed_seconds':round(elapsed,1),'browser_status':target['status']})
 if len(samples)%2==1:print(json.dumps({'event':'idle_observation','elapsed_seconds':round(elapsed,1),'browser_status':target['status']}),flush=True)
 time.sleep(30)
assert elapsed>=1795,'Browser closed before the configured 30-minute idle window'
owned=[]
for path in pathlib.Path('/proc').glob('[0-9]*/cmdline'):
 try:args=path.read_bytes().split(b'\0')
 except (FileNotFoundError,PermissionError,ProcessLookupError):continue
 if ('--user-data-dir=/data/browser-profiles/'+identity).encode() in args:owned.append(path.parent.name)
assert not owned,'Owned Chromium process remains after idle reclamation'
api('/api/browser-targets/'+identity,'DELETE');server.shutdown()
print(json.dumps({'event':'receipt','configured_seconds':1800,'actual_seconds':round(elapsed,1),'samples':samples,
                  'started_unix':start_time,'owned_browser_processes_after':0,'own_fixture_target_removed':True}),flush=True)
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
        data=json.loads(line)
        if data.get('event')=='receipt':receipt=data
    assert process.wait()==0 and receipt,'Idle reclamation acceptance failed'
    a.receipt.write_text(json.dumps(receipt,indent=2),encoding='utf-8')

if __name__=='__main__':main()
