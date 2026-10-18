"""Promote the verified diagnostics clone using docker run and a new data volume.

Only receipts from this acceptance workflow qualify. Original and acceptance
volumes are retained, and startup failure restores the original container.
"""
import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess
import time


COPY=r"""import hashlib,json,pathlib,shutil,stat
source=pathlib.Path('/source');target=pathlib.Path('/data');assert not any(target.iterdir())
def inventory(root):
 result={}
 for p in sorted(root.rglob('*')):
  info=p.lstat()
  if stat.S_ISDIR(info.st_mode):continue
  assert stat.S_ISREG(info.st_mode),'Unexpected special file: '+str(p)
  h=hashlib.sha256()
  with p.open('rb') as f:
   for b in iter(lambda:f.read(1024*1024),b''):h.update(b)
  result[p.relative_to(root).as_posix()]={'bytes':info.st_size,'sha256':h.hexdigest()}
 return result
original=inventory(source);shutil.copytree(source,target,dirs_exist_ok=True);copied=inventory(target)
assert original==copied,'Copied production data differs from the stopped acceptance volume'
print(json.dumps({'files':len(original),'bytes':sum(x['bytes'] for x in original.values()),'all_hashes_equal':True}))
"""

PREPARE=r"""import json,pathlib,shutil,sqlite3,sys,tomllib,uuid,os
sys.path.insert(0,'/app/tools');from gamer_docker import toml_document
baseline=set(json.load(sys.stdin));root=pathlib.Path('/data');db=sqlite3.connect(root/'gamer.db')
owned=[]
for identity,text in db.execute('select run_id,record from run_records'):
 record=json.loads(text)
 if record.get('content_package')=='diagnostics-acceptance-20261018':
  assert identity not in baseline and record.get('task_id') is None
  assert record['device_id'].startswith('browser-diagnostics-acceptance-20261018')
  uuid.UUID(identity);owned.append(identity)
assert len(owned)==9,owned
with db:
 for identity in owned:
  db.execute('delete from run_events where run_id=?',(identity,))
  db.execute('delete from run_records where run_id=?',(identity,))
watermark=json.loads((root/'diagnostics-policy.json').read_text())['legacy_log_max_id']
with db:db.execute("delete from logs where id>? and (device_id like 'browser-diagnostics-acceptance-20261018%' or script_id like 'diagnostics-acceptance-20261018/%')",(watermark,))
for identity in owned:
 path=root/'runtime-traces'/identity
 if path.exists():
  assert not path.is_symlink() and path.resolve().parent==(root/'runtime-traces').resolve()
  manifest=json.loads((path/'index.json').read_text())
  assert manifest.get('record',{}).get('content_package')=='diagnostics-acceptance-20261018'
  shutil.rmtree(path)
profiles=['browser-diagnostics-acceptance-20261018','browser-idle-acceptance-20261018']
profiles+=['browser-diagnostics-acceptance-20261018-parallel-'+str(i) for i in range(1,4)]
for identity in profiles:
 path=root/'browser-profiles'/identity
 if path.exists():
  assert not path.is_symlink() and path.resolve().parent==(root/'browser-profiles').resolve()
  shutil.rmtree(path)
assert db.execute('pragma integrity_check').fetchone()[0]=='ok'
assert db.execute('pragma user_version').fetchone()[0]==7
assert db.execute('select count(*) from timer_tasks where enabled=1').fetchone()[0]==4
assert db.execute('select count(*) from run_records').fetchone()[0]==len(baseline)+4
assert db.execute('select count(*) from browser_targets').fetchone()[0]==5
config=root/'config.toml';value=tomllib.loads(config.read_text());assert value['scheduler_enabled'] is False
value['scheduler_enabled']=True;text=toml_document(value);assert tomllib.loads(text)==value
temp=config.with_suffix('.promote.tmp');temp.write_text(text);temp.chmod(0o600);os.replace(temp,config)
print(json.dumps({'synthetic_runs_removed_from_new_production_only':owned,'acceptance_source_kept':True,'runs':len(baseline)+4,'enabled_tasks':4,'scheduler_enabled':True}))
"""

VERIFY=r"""import json,pathlib,sqlite3,tomllib,urllib.request
root=pathlib.Path('/data');token=(root/'state/local-admin-token').read_text().strip()
def get(path):return json.load(urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:8443'+path,headers={'X-Admin-Token':token}),timeout=30))
config=tomllib.loads((root/'config.toml').read_text());assert config['scheduler_enabled'] is True
assert config['browser_idle_secs']==1800
tasks=get('/api/tasks');assert len(tasks)==4 and all(x['enabled'] and x['state']=='active' and x['next_wakeup'] for x in tasks)
for task in tasks:assert get('/api/devices/'+task['app']['device_id']+'/run')['active'] is False
extensions=get('/api/extensions')['extensions'];yaml=next(x for x in extensions if x['id']=='gamer-yaml')
assert yaml['state']=='running' and yaml['version']=='0.3.23-local.20261018'
assert '0.3.21' in yaml['installed_versions']
assert sum(x['state']=='running' for x in extensions)==7
assert next(x for x in extensions if x['id']=='gamer-ai')['state']=='enabled'
db=sqlite3.connect('file:/data/gamer.db?mode=ro',uri=True);assert db.execute('pragma integrity_check').fetchone()[0]=='ok'
assert all(x['status']=='offline' for x in get('/api/browser-targets'))
server=[p for p in pathlib.Path('/proc').glob('[0-9]*') if (p/'comm').exists() and (p/'comm').read_text().strip()=='gamer-server'];assert len(server)==1
status=(server[0]/'status').read_text();assert 'Uid:\t1000\t1000\t1000\t1000' in status
print(json.dumps({'enabled_tasks':4,'scheduler_enabled':True,'next_wakeups':[x['next_wakeup'] for x in tasks],'running_plugins':7,'yaml_version':yaml['version'],'old_yaml_preserved':True,'db_integrity':'ok','runs':db.execute('select count(*) from run_records').fetchone()[0],'uid':1000,'online_browsers':0}))
"""


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--checkpoint',type=pathlib.Path,required=True)
    p.add_argument('--acceptance-container',required=True)
    p.add_argument('--production-container',required=True)
    p.add_argument('--rollback-container',required=True)
    p.add_argument('--production-volume',required=True)
    p.add_argument('--image-tag',required=True)
    a=p.parse_args();folder=a.checkpoint.resolve().parent;checkpoint=json.loads(a.checkpoint.read_text())
    assert a.acceptance_container.startswith('gamer-diagnostics-acceptance-')
    assert a.production_container==checkpoint['original_container']
    def receipt(name):return json.loads((folder/name).read_text())
    assert receipt('real-task-acceptance.json')['all_success']
    assert len(receipt('real-task-acceptance.json')['real_account_tasks'])==4
    assert len(receipt('fault-acceptance.json')['fault_cases'])==2
    assert receipt('parallel-acceptance.json')['all_slots_released']
    assert receipt('ocr-acceptance.json')['sampled_ocr_error_frame_preserved']
    assert not receipt('preservation-acceptance.json')['changed']
    assert receipt('download-lifecycle-acceptance.json')['temporary_files_remaining']==0
    assert receipt('idle-reclaim-acceptance.json')['actual_seconds']>=1795
    assert receipt('ui-acceptance.json')['download_confirmed_by_user'] is True
    observed=receipt('resource-observation.json')
    assert observed['completed'] and all(x['healthy'] and x['restarts']==0 for x in observed['samples'])
    assert receipt('rollback-rehearsal.json')['original_container_booted']
    archive=pathlib.Path(checkpoint['archive'])
    assert archive.is_file() and archive.stat().st_size==checkpoint['archive_bytes']
    digest=hashlib.sha256()
    with archive.open('rb') as stream:
        for block in iter(lambda:stream.read(1024*1024),b''):digest.update(block)
    assert digest.hexdigest()==checkpoint['archive_sha256'],'Checkpoint archive integrity changed'
    assert shutil.disk_usage(folder).free>checkpoint['verified_bytes']+4*1024**3
    def docker(*args):return subprocess.check_output(['docker',*args],text=True,encoding='utf-8').strip()
    def inspect(name):return json.loads(docker('inspect',name))[0]
    def healthy(name):
        for _ in range(90):
            state=inspect(name)['State']
            if state.get('Health',{}).get('Status')=='healthy':return
            assert state['Running'],'Service exited'
            time.sleep(1)
        raise TimeoutError('Production readiness failed')
    names=docker('ps','-a','--format','{{.Names}}').splitlines()
    assert a.rollback_container not in names
    assert a.production_volume not in docker('volume','ls','--format','{{.Name}}').splitlines()
    original=inspect(a.production_container);candidate=inspect(a.acceptance_container)
    assert not original['State']['Running'] and original['Image']==checkpoint['original_image']
    assert candidate['State']['Health']['Status']=='healthy' and candidate['RestartCount']==0
    assert candidate['Image']==json.loads(docker('image','inspect',checkpoint['candidate']))[0]['Id']
    assert any(x.get('Name')==checkpoint['restored_volume'] for x in candidate['Mounts'])
    repo=pathlib.Path(__file__).resolve().parents[1];env=repo/'.env';previous=folder/'deployment.env.before'
    assert env.read_bytes()==previous.read_bytes(),'Deployment selection changed since checkpoint'
    baseline_code="import sqlite3,json;d=sqlite3.connect('file:/data/gamer.db?mode=ro',uri=True);print(json.dumps([x[0] for x in d.execute('select run_id from run_records')]))"
    baseline=json.loads(docker('run','--rm','--memory','512m','--memory-swap','512m','--entrypoint','python3','--mount','type=volume,src='+checkpoint['original_volume']+',dst=/data,readonly',checkpoint['original_image'],'-c',baseline_code))
    files=receipt('file-manifest.json')['files']
    for prefix in ['browser-profiles/browser-diagnostics-acceptance-20261018','browser-profiles/browser-idle-acceptance-20261018']:
        assert not any(x.startswith(prefix) for x in files),'Synthetic profile name overlaps original data'
    preflight="""import pathlib,json,urllib.request,tomllib
root=pathlib.Path('/data');assert tomllib.loads((root/'config.toml').read_text())['scheduler_enabled'] is False
token=(root/'state/local-admin-token').read_text().strip()
def get(path):return json.load(urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:8443'+path,headers={'X-Admin-Token':token}),timeout=30))
for target in get('/api/devices'):assert get('/api/devices/'+target['id']+'/run')['active'] is False
assert all(target['status']=='offline' for target in get('/api/browser-targets'))
print('idle')
"""
    assert docker('exec',a.acceptance_container,'python3','-c',preflight)=='idle'
    renamed=False;wrote_env=False
    try:
        docker('stop','--time','90',a.acceptance_container)
        docker('volume','create','--label','gamer.role=production','--label','gamer.release=diagnostics-20261018',a.production_volume)
        copied=json.loads(docker('run','--rm','--memory','512m','--memory-swap','512m','--user','0','--entrypoint','python3',
            '--mount','type=volume,src='+checkpoint['restored_volume']+',dst=/source,readonly',
            '--mount','type=volume,src='+a.production_volume+',dst=/data',checkpoint['candidate'],'-c',COPY))
        (folder/'production-cold-copy.json').write_text(json.dumps(copied,indent=2),encoding='utf-8')
        print(json.dumps({'event':'cold_copy_verified',**copied}),flush=True)
        prepared=subprocess.check_output(['docker','run','--rm','--memory','512m','--memory-swap','512m','-i','--user','0','--entrypoint','python3',
            '--mount','type=volume,src='+a.production_volume+',dst=/data',checkpoint['candidate'],'-c',PREPARE],
            input=json.dumps(baseline),text=True,encoding='utf-8')
        prepared=json.loads(prepared)
        (folder/'production-prepared.json').write_text(json.dumps(prepared,indent=2),encoding='utf-8')
        print(json.dumps({'event':'production_data_prepared',**prepared}),flush=True)
        profile=folder/'chromium-seccomp.json';shutil.copy2(repo/'deploy/docker/chromium-seccomp.json',profile)
        docker('tag',checkpoint['candidate'],a.image_tag)
        docker('rename',a.production_container,a.rollback_container);renamed=True
        docker('run','-d','--name',a.production_container,'--label','gamer.role=production',
               '--label','gamer.release=diagnostics-20261018','--init','--restart','unless-stopped',
               '--stop-timeout','90','--shm-size','1g','--security-opt','seccomp='+str(profile),
               '--log-driver','json-file','--log-opt','max-size=10m','--log-opt','max-file=5',
               '-e','TZ=Asia/Shanghai','-e','TOKIO_WORKER_THREADS=2','-e','MALLOC_ARENA_MAX=2',
               '-p','127.0.0.1:18443:8443','--mount','type=volume,src='+a.production_volume+',dst=/data',a.image_tag)
        healthy(a.production_container)
        verified=json.loads(docker('exec',a.production_container,'python3','-c',VERIFY))
        assert inspect(a.production_container)['Image']==candidate['Image']
        selection=previous.read_text(encoding='utf-8').splitlines();updated=[]
        for line in selection:
            if line.startswith('GAMER_IMAGE='):line='GAMER_IMAGE='+a.image_tag
            elif line.startswith('GAMER_DATA_VOLUME='):line='GAMER_DATA_VOLUME='+a.production_volume
            updated.append(line)
        env.write_text('\n'.join(updated)+'\n',encoding='utf-8');wrote_env=True
        result={'production_container':a.production_container,'production_volume':a.production_volume,
                'image_tag':a.image_tag,'image_id':candidate['Image'],'rollback_container':a.rollback_container,
                'rollback_volume':checkpoint['original_volume'],'acceptance_volume_kept':checkpoint['restored_volume'],
                'copy':copied,'preparation':prepared,'verification':verified,'pushed_to_hub':False}
        (folder/'promotion.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
        shutil.copy2(repo/'tools/gamer_rollback.py',folder/'rollback.py')
        print(json.dumps({'event':'promoted',**result}),flush=True)
    except BaseException:
        if renamed and a.production_container in docker('ps','-a','--format','{{.Names}}').splitlines():
            partial=inspect(a.production_container)
            assert partial['Image']==candidate['Image'] and any(x.get('Name')==a.production_volume for x in partial['Mounts']), 'Unexpected container owns the production name; retained original was not altered'
            if partial['State']['Running']:docker('stop','--time','90',a.production_container)
            docker('rename',a.production_container,a.production_container+'-failed-'+str(int(time.time())))
        if renamed:docker('rename',a.rollback_container,a.production_container)
        docker('start',a.production_container);healthy(a.production_container)
        if wrote_env:env.write_bytes(previous.read_bytes())
        print('Promotion failed; original production restored. All volumes retained.',flush=True)
        raise

if __name__=='__main__':main()
