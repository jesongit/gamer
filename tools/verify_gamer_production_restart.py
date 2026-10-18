"""Restart the promoted local production once and verify persistent credentials.

Requires the finalized diagnostics cutover receipt and idle devices. This does
not run any task or connect any browser/game.
"""
import argparse
import datetime
import json
import pathlib
import subprocess
import time

from gamer_promote_diagnostics import VERIFY

SNAPSHOT = r"""import hashlib,json,pathlib,urllib.request
root=pathlib.Path('/data');token=(root/'state/local-admin-token').read_text().strip()
def get(path):return json.load(urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:8443'+path,headers={'X-Admin-Token':token}),timeout=30))
for device in get('/api/devices'):assert not get('/api/devices/'+device['id']+'/run')['active']
assert all(x['status']=='offline' for x in get('/api/browser-targets'))
selected={}
for path in root.rglob('*'):
 if path.is_symlink() or not path.is_file():continue
 relative=path.relative_to(root).as_posix()
 if ('/private/' in relative or relative in ['config.toml','state/credentials.key','state/local-admin-token']
  or (relative.startswith('browser-profiles/') and path.name in ['Cookies','Local State','Login Data'])):
  selected[relative]=hashlib.sha256(path.read_bytes()).hexdigest()
assert 'state/credentials.key' in selected and any('/private/' in x for x in selected)
print(json.dumps(selected))
"""

EXPORT = r"""import hashlib,io,json,pathlib,urllib.error,urllib.request,zipfile,sys
identity=sys.argv[1];url='http://127.0.0.1:8443/api/runs/'+identity+'/diagnostics'
try:
 urllib.request.urlopen(url,timeout=30).close();raise AssertionError('Anonymous export accepted')
except urllib.error.HTTPError as error:assert error.code==401,error.code
token=pathlib.Path('/data/state/local-admin-token').read_text().strip()
with urllib.request.urlopen(urllib.request.Request(url,headers={'X-Admin-Token':token}),timeout=90) as response:
 assert response.status==200;raw=response.read(160*1024*1024)
with zipfile.ZipFile(io.BytesIO(raw)) as archive:
 assert archive.testzip() is None
 manifest=json.loads(archive.read('manifest.json'));assert manifest['run_id']==identity
 for item in manifest['assets']:assert hashlib.sha256(archive.read(item['path'])).hexdigest()==item['sha256']
 print(json.dumps({'run_id':identity,'status':200,'anonymous_status':401,'zip_bytes':len(raw),'asset_hashes':'passed'}))
"""

RESOURCES = r"""import json,pathlib
counts={'gamer-server':0,'chrome':0,'node':0,'adb':0};server=None
for proc in pathlib.Path('/proc').glob('[0-9]*'):
 try:
  command=(proc/'cmdline').read_bytes().split(b'\0')[0].decode();name=pathlib.Path(command).name
  if name=='gamer-server':counts['gamer-server']+=1;server=proc
  elif name in ['chrome','chromium','chrome_crashpad_handler','chromium_crashpad_handler']:counts['chrome']+=1
  elif name in ['node','nodejs']:counts['node']+=1
  elif name=='adb':counts['adb']+=1
 except (FileNotFoundError,PermissionError,ProcessLookupError):pass
assert counts['gamer-server']==1 and counts['chrome']==counts['node']==0,counts
status={a:b.strip() for a,b in (line.split(':',1) for line in (server/'status').read_text().splitlines() if ':' in line)}
result={'processes':counts,'server_rss_kib':int(status['VmRSS'].split()[0])}
current=pathlib.Path('/sys/fs/cgroup/memory.current');stat=pathlib.Path('/sys/fs/cgroup/memory.stat')
if current.exists():result['cgroup_current_bytes']=int(current.read_text())
if stat.exists():result['cgroup_memory_stat']={k:int(v) for k,v in (line.split() for line in stat.read_text().splitlines()) if k in ['anon','file','inactive_file','active_file','kernel']}
print(json.dumps(result))
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--checkpoint', type=pathlib.Path, required=True)
    args = parser.parse_args()
    folder = args.checkpoint.resolve().parent
    promotion = json.loads((folder / 'promotion.json').read_text())
    name = promotion['production_container']
    assert name == 'gamer-production-gamer-1'
    receipt_path = folder / 'production-restart.json'
    assert not receipt_path.exists(), 'Restart already verified'

    def docker(*values):
        return subprocess.check_output(['docker', *values], text=True, encoding='utf-8').strip()

    def inspect():
        return json.loads(docker('inspect', name))[0]

    initial = inspect()
    assert initial['Image'] == promotion['image_id']
    assert any(x.get('Name') == promotion['production_volume'] for x in initial['Mounts'])
    assert initial['HostConfig']['RestartPolicy']['Name'] == 'unless-stopped'
    assert initial['HostConfig']['LogConfig']['Config'] == {'max-file':'5','max-size':'10m'}
    before = json.loads(docker('exec', name, 'python3', '-c', SNAPSHOT))
    print(json.dumps({'event':'persistent_fingerprints_captured','files':len(before)}), flush=True)
    docker('restart', '--time', '90', name)
    for _ in range(90):
        current = inspect()
        assert current['State']['Running'], 'Production exited after restart'
        if current['State'].get('Health', {}).get('Status') == 'healthy':
            break
        time.sleep(1)
    else:
        raise TimeoutError('Production readiness after restart')
    assert current['State']['StartedAt'] != initial['State']['StartedAt']
    verified = json.loads(docker('exec', name, 'python3', '-c', VERIFY))
    after = json.loads(docker('exec', name, 'python3', '-c', SNAPSHOT))
    assert before == after, {'changed_persistent_files':[key for key in before.keys()|after.keys() if before.get(key)!=after.get(key)]}
    real = json.loads((folder / 'real-task-acceptance.json').read_text())['real_account_tasks'][0]['run_id']
    export = json.loads(docker('exec', name, 'python3', '-c', EXPORT, real))
    resources = json.loads(docker('exec', name, 'python3', '-c', RESOURCES))
    final = inspect()
    assert final['State']['Health']['Status'] == 'healthy' and final['RestartCount'] == 0
    result = {'verified_at_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'production_container':name,'manual_restart_verified':True,
              'persistent_files_unchanged':len(before),'persistent_file_hashes':after,
              'verification':verified,'diagnostics_download':export,'resources_after_restart':resources,
              'automatic_restarts':final['RestartCount'],'task_execution_triggered':False}
    receipt_path.write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps({k:v for k,v in result.items() if k!='persistent_file_hashes'}))


if __name__ == '__main__':
    main()
