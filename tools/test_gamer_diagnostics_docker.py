"""Opt-in Linux image integration test with disposable, synthetic data only.

Usage: python tools/test_gamer_diagnostics_docker.py --image gamer:diagnostics-dev-20261018
Never mounts production or host browser data and never publishes a service port.
"""
import argparse
import json
import pathlib
import subprocess
import time
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True)
    parser.add_argument("--receipt", type=pathlib.Path)
    args = parser.parse_args()
    suffix = uuid.uuid4().hex[:10]
    container = "gamer-diagnostics-test-" + suffix
    volume = container + "-data"
    label = "codex.gamer-diagnostics-test=" + suffix
    print(json.dumps({"isolated_container": container, "isolated_volume": volume}), flush=True)

    def docker(*values, capture=False):
        return subprocess.run(["docker", *map(str, values)], check=True, text=True,
                              stdout=subprocess.PIPE if capture else subprocess.DEVNULL,
                              timeout=120).stdout

    def helper(code):
        return docker("run", "--rm", "--user", "0", "--entrypoint", "python3",
                      "--mount", "type=volume,src=" + volume + ",dst=/data",
                      args.image, "-c", code, capture=True)

    def ready():
        for _ in range(40):
            state = docker("inspect", container, "--format", "{{.State.Health.Status}}", capture=True).strip()
            if state == "healthy":
                return
            time.sleep(1)
        raise RuntimeError("Isolated service did not become healthy")

    docker("image", "inspect", args.image)
    docker("volume", "create", "--label", label, volume)
    helper("""import sys,argparse
sys.path.insert(0,'/app/tools')
import gamer_docker
gamer_docker.getpass.getpass=lambda prompt:'Synthetic-fixture-only-2026'
gamer_docker.initialize(argparse.Namespace(data='/data'))
""")
    profile = pathlib.Path(__file__).resolve().parents[1] / "deploy/docker/chromium-seccomp.json"
    docker("run", "-d", "--name", container, "--label", label,
           "--init", "--stop-timeout", "90", "--shm-size", "1g",
           "--security-opt", "seccomp=" + str(profile),
           "-e", "TOKIO_WORKER_THREADS=2", "-e", "MALLOC_ARENA_MAX=2",
           "--mount", "type=volume,src=" + volume + ",dst=/data", args.image)
    ready()
    docker("stop", "--time", "90", container)
    helper("""import pathlib,sqlite3,json,datetime,hashlib,struct,zlib,os
root=pathlib.Path('/data');now=datetime.datetime.now(datetime.timezone.utc)
def at(days):return (now-datetime.timedelta(days=days)).isoformat()
def png():
 raw=b''.join(b'\\0'+bytes([25,2,3])*20 for _ in range(10))
 def chunk(kind,data):return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data)&0xffffffff)
 return b'\\x89PNG\\r\\n\\x1a\\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',20,10,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(raw))+chunk(b'IEND',b'')
db=sqlite3.connect(root/'gamer.db')
for identity,state,days,managed in [('legacy-summary','success',500,False),('new-success','success',20,True),('new-failed-old','failed',20,True),('new-valid','failed',2,True),('legacy-trace','failed',500,False)]:
 record={'run_id':identity,'device_id':'synthetic','runner_id':'fixture','entrypoint':'fixture/daily','content_package':'fixture','script_id':'fixture/daily','source':'manual','state':state,'started_at':at(days),'finished_at':at(days),'error':'FIND_TIMEOUT' if state=='failed' else None}
 if managed:record['_diagnostics_version']=1
 db.execute('INSERT INTO run_records(run_id,device_id,entrypoint,started_at,finished_at,record) VALUES(?,?,?,?,?,?)',(identity,'synthetic','fixture/daily',at(days),at(days),json.dumps(record)))
 payload={'ev':'detail','name':'fixture','data':{'api_key':'DO_NOT_EXPORT_KEY','args':{'password':'DO_NOT_EXPORT_ARGUMENT'}}}
 db.execute('INSERT INTO run_events(run_id,time,payload) VALUES(?,?,?)',(identity,at(days),json.dumps(payload)))
 if identity not in ['new-valid','legacy-trace']:continue
 directory=root/'runtime-traces'/identity;directory.mkdir()
 asset=hashlib.sha256(struct.pack('<II',20,10)+bytes([25,2,3])*200).hexdigest()+'.png'
 (directory/asset).write_bytes(png())
 image={'seq':1,'image_id':'fixture-image','captured_at':at(days),'recorded_at':at(days),'width':20,'height':10,'kind':'error_last_consumed','source':{'target':'synthetic','epoch':'fixture-epoch','revision':1},'metadata':{'error':'FIND_TIMEOUT'},'url':'/unused','asset':asset}
 manifest={'run_id':identity,'status':'available','enabled':True,'started_at':at(days),'finished_at':at(days),'bytes':2048,'metadata_bytes':1024,'gaps':[],'images':[image],'snapshot':{'_source_files':{'fixture':'DO_NOT_EXPORT_SOURCE'}},'record':record}
 if managed:manifest['policy_version']=1
 content=json.dumps(manifest).encode();(directory/'index.json').write_bytes(content)
 for item in directory.iterdir():os.chown(item,1000,1000)
 os.chown(directory,1000,1000)
 if not managed:(root/'fixture-original-sha256').write_text(hashlib.sha256(content).hexdigest())
db.execute("INSERT INTO logs(time,device_id,script_id,level,msg) VALUES('2000-01-01','fixture','fixture','info','legacy-original')")
db.commit();db.close()
(root/'diagnostics-policy.json').unlink() # Only the synthetic pre-policy fixture.
""")
    docker("start", container)
    ready()
    result = docker("exec", container, "python3", "-c", """import pathlib,json,sqlite3,urllib.request,urllib.error,io,zipfile,hashlib
root=pathlib.Path('/data');token=(root/'state/local-admin-token').read_text().strip()
def request(path,authenticated=True):
 req=urllib.request.Request('http://127.0.0.1:8443'+path,headers={'X-Admin-Token':token} if authenticated else {})
 return urllib.request.urlopen(req,timeout=20)
try:request('/api/runs/new-valid/diagnostics',False);raise AssertionError('Unauthenticated export accepted')
except urllib.error.HTTPError as error:assert error.code==401
for identity,expected in [('new-success','expired'),('new-failed-old','available'),('legacy-summary','available')]:
 with request('/api/runs/'+identity+'/events') as response:data=json.load(response)
 assert data['status']==expected,(identity,data)
 assert len(data['events'])==(0 if expected=='expired' else 1)
with request('/api/runs/new-valid/diagnostics') as response:
 assert response.headers['Cache-Control']=='private, no-store'
 content=response.read(2*1024*1024)
with zipfile.ZipFile(io.BytesIO(content)) as archive:
 assert archive.testzip() is None
 for name in archive.namelist():
  value=archive.read(name)
  for secret in [b'DO_NOT_EXPORT_KEY',b'DO_NOT_EXPORT_ARGUMENT',b'DO_NOT_EXPORT_SOURCE']:assert secret not in value
 assert any(name.startswith('assets/') for name in archive.namelist())
 manifest=json.loads(archive.read('manifest.json'));assert manifest['run_id']=='new-valid'
legacy=root/'runtime-traces/legacy-trace/index.json'
assert hashlib.sha256(legacy.read_bytes()).hexdigest()==(root/'fixture-original-sha256').read_text()
checkpoint=json.loads((root/'diagnostics-policy.json').read_text());assert checkpoint['legacy_log_max_id']==1
db=sqlite3.connect(root/'gamer.db');assert db.execute('PRAGMA integrity_check').fetchone()[0]=='ok'
assert db.execute('PRAGMA user_version').fetchone()[0]==7
assert db.execute('SELECT count(*) FROM timer_tasks').fetchone()[0]==0
assert db.execute('SELECT count(*) FROM logs').fetchone()[0]==1
db.execute("INSERT INTO logs(time,device_id,script_id,level,msg) VALUES('2000-01-01','fixture','fixture','info','new-expired')");db.commit()
print(json.dumps({'schema':7,'archive_bytes':len(content),'auth':'passed','redaction':'passed','retention':'passed','legacy_pixels_and_index':'unchanged','synthetic_tasks':0}))
""", capture=True)
    docker("stop", "--time", "90", container)
    docker("start", container)
    ready()
    docker("exec", container, "python3", "-c", """import sqlite3,json,pathlib,hashlib
root=pathlib.Path('/data');db=sqlite3.connect(root/'gamer.db')
assert db.execute('SELECT msg FROM logs ORDER BY id').fetchall()==[('legacy-original',)]
assert hashlib.sha256((root/'runtime-traces/legacy-trace/index.json').read_bytes()).hexdigest()==(root/'fixture-original-sha256').read_text()
""")
    receipt = json.loads(result)
    receipt.update({"image": args.image, "image_id": docker("inspect", container, "--format", "{{.Image}}", capture=True).strip(),
                    "restart": "passed", "production_data_used": False})
    docker("stop", "--time", "90", container)
    assert docker("inspect", container, "--format", '{{index .Config.Labels "codex.gamer-diagnostics-test"}}', capture=True).strip() == suffix
    docker("rm", container)
    assert docker("volume", "inspect", volume, "--format", '{{index .Labels "codex.gamer-diagnostics-test"}}', capture=True).strip() == suffix
    docker("volume", "rm", volume)
    receipt["fixture_resources_removed"] = True
    if args.receipt:
        args.receipt.parent.mkdir(parents=True, exist_ok=True)
        args.receipt.write_text(json.dumps(receipt, indent=2), encoding="utf-8")
    print(json.dumps(receipt))


if __name__ == "__main__":
    main()
