"""Compare protected pre-upgrade files and original SQLite rows to a test clone."""
import argparse
import json
import pathlib
import subprocess

ROWS=r"""import hashlib,json,pathlib,sqlite3,sys
db=sqlite3.connect('file:/data/gamer.db?mode=ro',uri=True);db.row_factory=sqlite3.Row
watermark=int(sys.argv[1]);result={}
for table,key in [('run_records','run_id'),('run_events','id'),('logs','id'),('devices','id'),('browser_targets','id'),('task_presets','id'),('timer_tasks','id')]:
 result[table]={}
 query='select * from '+table+(' where id<='+str(watermark) if table=='logs' else '')
 for item in db.execute(query):
  row=dict(item)
  if table=='timer_tasks':
   for field in ['state','next_wakeup','last_result','last_run_at','updated_at','suspend_reason']:row.pop(field,None)
  value=json.dumps(row,sort_keys=True,separators=(',',':')).encode()
  result[table][str(item[key])]=hashlib.sha256(value).hexdigest()
history=pathlib.Path('/data/extension-data/gamer-notify/records.json')
result['notification_history']={item['id']:hashlib.sha256(json.dumps(item,sort_keys=True,separators=(',',':')).encode()).hexdigest() for item in json.loads(history.read_text())}
queue=pathlib.Path('/data/extension-data/gamer-live/queue.json')
result['live_queue']={'document':hashlib.sha256(json.dumps(json.loads(queue.read_text()),sort_keys=True,separators=(',',':')).encode()).hexdigest()}
print(json.dumps(result))
"""

FILES=r"""import hashlib,json,pathlib,sys
manifest=json.load(sys.stdin);root=pathlib.Path('/data');changed=[];total=0
for relative,expected in manifest.items():
 path=root/relative
 if path.is_symlink() or not path.is_file():changed.append({'path':relative,'reason':'missing_or_symlink'});continue
 h=hashlib.sha256()
 with path.open('rb') as f:
  for block in iter(lambda:f.read(1024*1024),b''):h.update(block)
 if path.stat().st_size!=expected['bytes'] or h.hexdigest()!=expected['sha256']:changed.append({'path':relative,'reason':'bytes_changed'})
 total+=expected['bytes']
print(json.dumps({'protected_files':len(manifest),'protected_bytes':total,'changed':changed}))
"""

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--checkpoint',type=pathlib.Path,required=True)
    p.add_argument('--candidate-container',required=True)
    p.add_argument('--original-container')
    p.add_argument('--allow-production-read-only',action='store_true')
    p.add_argument('--receipt',type=pathlib.Path,required=True)
    a=p.parse_args();checkpoint=json.loads(a.checkpoint.read_text())
    assert a.candidate_container.startswith('gamer-diagnostics-acceptance-') or (a.allow_production_read_only and a.candidate_container=='gamer-production-gamer-1')
    manifest=json.loads((a.checkpoint.parent/'file-manifest.json').read_text())['files']
    mutable={'config.toml','auth-sessions.json','extensions/state.json','gamer.db','gamer.db-wal','gamer.db-shm','extension-data/gamer-notify/records.json','extension-data/gamer-live/queue.json'}
    mutable_prefixes=('browser-profiles/','state/home/','mcp/')
    selected={k:v for k,v in manifest.items() if k not in mutable and not k.startswith(mutable_prefixes)}
    candidate_state=json.loads(subprocess.check_output(['docker','inspect',a.candidate_container]))[0]
    mounts=[x for x in candidate_state['Mounts'] if x['Destination']=='/data']
    assert len(mounts)==1 and mounts[0]['Type']=='volume'
    # Filesystem cache is charged to this bounded helper, never to production.
    candidate_helper=['docker','run','--rm','--memory','512m','--memory-swap','512m','--user','0','--entrypoint','python3',
                      '--mount','type=volume,src='+mounts[0]['Name']+',dst=/data,readonly',candidate_state['Image']]
    result=subprocess.run(candidate_helper[:3]+['-i']+candidate_helper[3:]+['-c',FILES],
                          input=json.dumps(selected),text=True,encoding='utf-8',capture_output=True,check=True)
    files=json.loads(result.stdout)
    assert not files['changed'],files['changed']
    watermark=int(subprocess.check_output(['docker','exec',a.candidate_container,'python3','-c',
        "import json,pathlib;print(json.loads(pathlib.Path('/data/diagnostics-policy.json').read_text())['legacy_log_max_id'])"],text=True))
    original_name=a.original_container or checkpoint['original_container']
    original_state=json.loads(subprocess.check_output(['docker','inspect',original_name]))[0]
    assert original_state['State']['Running'] is False,'Original must stay stopped for a stable comparison'
    assert original_state['Image']==checkpoint['original_image']
    assert any(x.get('Name')==checkpoint['original_volume'] for x in original_state['Mounts'])
    original=json.loads(subprocess.check_output(['docker','run','--rm','--memory','512m','--memory-swap','512m','--user','0','--entrypoint','python3',
        '--mount','type=volume,src='+checkpoint['original_volume']+',dst=/data,readonly',checkpoint['original_image'],
        '-c',ROWS,str(watermark)],text=True,encoding='utf-8'))
    candidate=json.loads(subprocess.check_output(candidate_helper+['-c',ROWS,str(watermark)],text=True,encoding='utf-8'))
    counts={}
    for table,rows in original.items():
        missing=[key for key,value in rows.items() if candidate[table].get(key)!=value]
        assert not missing,{'table':table,'changed_original_rows':missing[:5]}
        counts[table]=len(rows)
    receipt={**files,'original_rows_unchanged':counts,'legacy_log_watermark':watermark,
             'mutable_runtime_files_excluded':sorted(mutable),'mutable_runtime_prefixes_excluded':list(mutable_prefixes),
             'live_queue_comparison':'complete canonical JSON digest; startup may reorder object keys'}
    a.receipt.write_text(json.dumps(receipt,indent=2),encoding='utf-8')
    print(json.dumps(receipt))

if __name__=='__main__':main()
