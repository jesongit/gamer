"""Interrupted/overlapping diagnostic exports must release capacity and files."""
import argparse
import json
import pathlib
import subprocess

CHECK=r"""import hashlib,io,json,pathlib,time,urllib.error,urllib.request,zipfile
identity=__import__('sys').argv[1];token=pathlib.Path('/data/state/local-admin-token').read_text().strip()
def download():
 return urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:8443/api/runs/'+identity+'/diagnostics',headers={'X-Admin-Token':token}),timeout=120)
pid=next(p for p in pathlib.Path('/proc').glob('[0-9]*') if (p/'comm').exists() and (p/'comm').read_text().strip()=='gamer-server')
def resources():
 values={a:b.strip() for a,b in (line.split(':',1) for line in (pid/'status').read_text().splitlines() if ':' in line)}
 return {'rss_kib':int(values['VmRSS'].split()[0]),'fds':len(list((pid/'fd').iterdir()))}
baseline=resources();before=set(pathlib.Path('/tmp').glob('.tmp*'));results=[]
for index in range(4):
 first=download();first.read(1024)
 try:
  second=download();second.close();raise AssertionError('Overlapping export was accepted')
 except urllib.error.HTTPError as error:assert error.code==429,error.code
 first.close()
 for attempt in range(30):
  try:
   complete=download();break
  except urllib.error.HTTPError as error:
   assert error.code==429;time.sleep(.1)
 else:raise AssertionError('Export permit was not released after disconnect')
 with complete:raw=complete.read(160*1024*1024)
 with zipfile.ZipFile(io.BytesIO(raw)) as archive:
  assert archive.testzip() is None
  manifest=json.loads(archive.read('manifest.json'));assert manifest['run_id']==identity
  for item in manifest['assets']:
   assert hashlib.sha256(archive.read(item['path'])).hexdigest()==item['sha256']
 results.append({'cycle':index+1,'zip_bytes':len(raw),'overlap_status':429,'retry_status':200})
 for _ in range(30):
  if not (set(pathlib.Path('/tmp').glob('.tmp*'))-before):break
  time.sleep(.1)
 assert not (set(pathlib.Path('/tmp').glob('.tmp*'))-before),'Temporary export remains after download'
after=resources();assert after['fds']<=baseline['fds']+3,(baseline,after)
assert after['rss_kib']<=baseline['rss_kib']+96*1024,(baseline,after)
print(json.dumps({'run_id':identity,'cycles':results,'before':baseline,'after':after,'temporary_files_remaining':0,'asset_hashes':'passed'}))
"""

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--container',required=True)
    p.add_argument('--run-id',required=True)
    p.add_argument('--receipt',type=pathlib.Path,required=True)
    a=p.parse_args();assert a.container.startswith('gamer-diagnostics-acceptance-')
    result=subprocess.check_output(['docker','exec',a.container,'python3','-c',CHECK,a.run_id],text=True,encoding='utf-8')
    receipt=json.loads(result);a.receipt.write_text(json.dumps(receipt,indent=2),encoding='utf-8');print(result.strip())

if __name__=='__main__':main()
