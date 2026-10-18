"""Cold, full-volume checkpoint and verified restore before a Gamer upgrade.

Leaves the old container and original volume intact. On checkpoint failure the
old service is restarted automatically; a restored volume is never deleted.
"""
import argparse
import hashlib
import json
import pathlib
import subprocess
import sys

from gamer_docker import private_permissions


INVENTORY = r"""import hashlib,json,pathlib,sqlite3,stat
root=pathlib.Path('/data'); result={}
for path in sorted(root.rglob('*')):
 info=path.lstat()
 if stat.S_ISREG(info.st_mode):
  h=hashlib.sha256()
  with path.open('rb') as f:
   for chunk in iter(lambda:f.read(1024*1024),b''):h.update(chunk)
  result[path.relative_to(root).as_posix()]={'bytes':info.st_size,'sha256':h.hexdigest()}
 elif not stat.S_ISDIR(info.st_mode):raise ValueError('Unexpected special file: '+str(path))
db=sqlite3.connect('file:/data/gamer.db?mode=ro',uri=True)
assert db.execute('pragma integrity_check').fetchone()[0]=='ok'
counts={table:db.execute('select count(*) from '+table).fetchone()[0] for table in ('timer_tasks','run_records','run_events','logs','browser_targets')}
assert db.execute('pragma user_version').fetchone()[0]==7
print(json.dumps({'files':result,'schema':7,'counts':counts}))
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--container', required=True)
    parser.add_argument('--candidate', required=True)
    parser.add_argument('--destination', type=pathlib.Path, required=True)
    parser.add_argument('--restored-volume', required=True)
    args = parser.parse_args()
    dest = args.destination.resolve()
    dest.mkdir(parents=True, exist_ok=False)
    private_permissions(dest, directory=True)

    def docker(*values):
        return subprocess.check_output(['docker', *values], text=True, encoding='utf-8').strip()

    state = json.loads(docker('inspect', args.container))[0]
    mounts = [x for x in state['Mounts'] if x['Destination'] == '/data']
    assert len(mounts) == 1 and mounts[0]['Type'] == 'volume'
    assert state['State']['Running'] and state['State']['Health']['Status'] == 'healthy'
    volume = mounts[0]['Name']
    old_image = state['Image']
    docker('image', 'inspect', args.candidate)
    assert not docker('ps', '-q', '--filter', 'volume='+args.restored_volume)
    known = docker('volume', 'ls', '--format', '{{.Name}}').splitlines()
    assert args.restored_volume not in known, 'Restore volume must be new'
    (dest/'container-inspect.json').write_text(json.dumps(state, indent=2), encoding='utf-8')
    env = pathlib.Path(__file__).resolve().parents[1]/'.env'
    if env.exists():
        (dest/'deployment.env.before').write_bytes(env.read_bytes())
    for path in dest.iterdir():
        private_permissions(path)

    preflight = r"""import json,pathlib,urllib.request
token=pathlib.Path('/data/state/local-admin-token').read_text().strip()
def get(p):return json.load(urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:8443'+p,headers={'X-Admin-Token':token}),timeout=15))
tasks=get('/api/tasks');assert len(tasks)==4 and all(t['enabled'] for t in tasks)
for task in tasks:assert get('/api/devices/'+task['app']['device_id']+'/run')['active'] is False
assert all(t['status']=='offline' for t in get('/api/browser-targets'))
print(json.dumps({'tasks':4,'active_runs':0,'online_browsers':0}))
"""
    print(docker('exec', args.container, 'python3', '-c', preflight), flush=True)
    stopped = False
    try:
        docker('stop', '--time', '90', args.container)
        stopped = True
        print('Old container stopped cleanly; taking full checkpoint.', flush=True)
        source = json.loads(docker('run', '--rm', '--memory', '512m', '--memory-swap', '512m', '--user', '0', '--entrypoint', 'python3',
                                  '--mount', 'type=volume,src='+volume+',dst=/data,readonly',
                                  old_image, '-c', INVENTORY))
        (dest/'file-manifest.json').write_text(json.dumps(source), encoding='utf-8')
        private_permissions(dest/'file-manifest.json')
        archive = dest/'data-full.tar.gz'
        helper = pathlib.Path(__file__).with_name('gamer_docker.py')
        subprocess.run([sys.executable, str(helper), 'backup', '--volume', volume,
                        '--image', old_image, '--profile', 'full', '--output', str(archive)], check=True)
        h=hashlib.sha256()
        with archive.open('rb') as f:
            for block in iter(lambda:f.read(1024*1024), b''):h.update(block)
        subprocess.run([sys.executable, str(helper), 'restore', '--volume', args.restored_volume,
                        '--image', args.candidate, '--input', str(archive)], check=True)
        restored=json.loads(docker('run', '--rm', '--memory', '512m', '--memory-swap', '512m', '--user', '0', '--entrypoint', 'python3',
                                   '--mount', 'type=volume,src='+args.restored_volume+',dst=/data,readonly',
                                   args.candidate, '-c', INVENTORY))
        assert source == restored, 'Restored file hashes or database differ from stopped source'
        receipt={'original_container':args.container,'original_volume':volume,'original_image':old_image,
                 'candidate':args.candidate,'restored_volume':args.restored_volume,
                 'archive':str(archive),'archive_sha256':h.hexdigest(),'archive_bytes':archive.stat().st_size,
                 'verified_files':len(source['files']),'verified_bytes':sum(f['bytes'] for f in source['files'].values()),
                 'database':source['counts'],'schema':source['schema'],'restored_hashes':'all_equal',
                 'old_container_stopped':True}
        (dest/'checkpoint.json').write_text(json.dumps(receipt,indent=2),encoding='utf-8')
        private_permissions(dest/'checkpoint.json')
        print(json.dumps(receipt), flush=True)
    except BaseException:
        if stopped:
            docker('start', args.container)
            print('Checkpoint failed; original service restarted. All data retained.', flush=True)
        raise


if __name__=='__main__':
    main()
