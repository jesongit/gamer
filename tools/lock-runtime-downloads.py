"""Maintainer-only: record immutable runtime artifacts; never run at server startup."""
from pathlib import Path
import hashlib, json, urllib.request
root = Path(__file__).resolve().parent.parent
cache = root/'server/target/runtime-downloads'
cache.mkdir(parents=True,exist_ok=True)
items=[]
for platform in ('win64','linux64'):
    items.append(dict(id='browser',platform=platform,version='155.0.8059.84',url=f'https://storage.googleapis.com/chrome-for-testing-public/155.0.8059.84/{platform}/chrome-{platform}.zip',executable=f'chrome-{platform}/'+('chrome.exe' if platform=='win64' else 'chrome')))
items += [dict(id='7zr',platform='win64',version='26.04',url='https://www.7-zip.org/a/7zr.exe'),dict(id='7z-package',platform='win64',version='25.01',url='https://www.7-zip.org/a/7z2501-x64.exe')]
for item in items:
    path=cache/(item['platform']+'-'+item['url'].split('/')[-1])
    if not path.exists():
        with urllib.request.urlopen(item['url'],timeout=60) as r,path.open('wb') as w:
            while b:=r.read(1024*1024): w.write(b)
    item['size']=path.stat().st_size
    with path.open('rb') as f:item['sha256']=hashlib.file_digest(f,'sha256').hexdigest()
    print(item['id'],item['platform'],item['size'],flush=True)
(root/'release/runtime/artifacts.lock.json').write_text(json.dumps(items,indent=2)+'\n',encoding='utf-8')
