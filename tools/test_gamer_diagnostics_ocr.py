"""Real Linux OCR must remain the error frame when its ordinary image was sampled."""
import argparse
import json
import pathlib
import subprocess

from test_gamer_diagnostics_fault import CHECK as FAULT_CHECK

CHECK=FAULT_CHECK.split('results=[]\n',1)[0]+r"""
def ocr_page(self):
 body=b'''<html><body style="margin:0;background:#18324c;color:white;font:60px monospace">Initial<script>setTimeout(()=>{document.body.style.background='white';document.body.style.color='black';document.body.innerHTML='<div style="padding:20px">123 / 240</div>';},2500)</script></body></html>'''
 self.send_response(200);self.send_header('Content-Type','text/html');self.send_header('Cache-Control','no-store');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
Fixture.do_GET=ocr_page
source='''name: Synthetic sampled OCR evidence
run:
  - find: {template: absent.png, threshold: 0.99}
  - sleep: 4s
  - ocr: {region: [0, 0, 1, 0.5], language: eng, mode: line, whitelist: "0123456789/"}
    as: digits
  - number: {text: $digits.text}
    as: recognized_number
  - eq: {a: $recognized_number, b: 123}
    as: recognized
  - if: $recognized
    then:
      - fail: {message: Synthetic expected failure after OCR, code: SYNTHETIC_OCR_EXPECTED}
    else:
      - fail: {message: Unexpected OCR result, code: SYNTHETIC_OCR_UNEXPECTED}
'''
api(prefix+'automations/ocr.yaml','PUT',{'content':source})
identity=api('/api/runs','POST',{'runner_id':'gamer-yaml','entrypoint':pkg+'/ocr.yaml','device_id':device,'content_package':pkg})['run_id']
print(json.dumps({'event':'started','run_id':identity}),flush=True);start=time.monotonic()
while True:
 record=api('/api/runs/'+identity)
 if record['state'] in ('success','failed','cancelled'):break
 assert time.monotonic()-start<60
 time.sleep(.2)
assert record['state']=='failed' and 'SYNTHETIC_OCR_EXPECTED' in record['error'],record.get('error')
trace=api('/api/runs/'+identity+'/trace?limit=100');images=trace['images']
assert trace['sampled_out']>=1 and not any(i['kind']=='ocr' for i in images)
last=next(i for i in images if i['kind']=='error_last_consumed');fresh=next(i for i in images if i['kind']=='error_fresh')
assert last['metadata']['path']=='run[2]' and last['metadata']['region']==[0,0,640,180],last['metadata']
initial=next(i for i in images if i['kind']=='consumed')
with request(initial['url']) as r:initial_hash=hashlib.sha256(r.read()).hexdigest()
with request(last['url']) as r:ocr_hash=hashlib.sha256(r.read()).hexdigest()
assert initial_hash!=ocr_hash,'OCR evidence reused the earlier NCC frame'
for _ in range(20):
 if not api('/api/devices/'+device+'/run')['active']:break
 time.sleep(.2)
assert not api('/api/devices/'+device+'/run')['active']
api('/api/browser-targets/'+device+'/disconnect','POST',{});api('/api/browser-targets/'+device,'DELETE');api('/api/packages/'+pkg,'DELETE');server.shutdown()
print(json.dumps({'event':'receipt','run_id':identity,'linux_tesseract_recognized_number':123,
                  'sampled_out':trace['sampled_out'],'sampled_ocr_error_frame_preserved':True,
                  'ocr_error_frame_path':last['metadata']['path'],'ocr_roi':last['metadata']['region'],
                  'initial_frame_sha256':initial_hash,'ocr_frame_sha256':ocr_hash,'fresh_error_frame_present':True,
                  'own_fixture_target_removed':True}),flush=True)
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
    assert process.wait()==0 and receipt,'OCR error evidence acceptance failed'
    a.receipt.write_text(json.dumps(receipt,indent=2),encoding='utf-8')

if __name__=='__main__':main()
