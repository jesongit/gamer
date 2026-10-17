// Local browser fixture. Recorded PNGs provide visual evidence; these synthetic
// state transitions never connect to miHoYo, a real account or a live platform.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';

export async function createRecordedLoginFixture(evidenceDirectory, desktopPopupPath) {
  const pictures = {
    expired: 'expired-index-0.png', qr: 'login-index-0.png',
    enter: 'login-index-3.png', menu: 'login-index-4.png',
    confirm: 'login-index-17.png', scan: 'login-index-76.png',
  };
  const imageBytes = new Map(await Promise.all(Object.entries(pictures).map(async ([phase, file]) => [phase, await readFile(resolve(evidenceDirectory, file))])));
  imageBytes.set('desktop_popup', await readFile(desktopPopupPath));
  let control = { epoch: 1, phase: 'scan', autoLogin: true, loginDelayMs: 1800, expireFirstScan: true, desktopPopup: true, requireMoreClick: false, requireMoreClickOnEvenCycle: true };
  const events = [];
  let lastPage = null;
  const json = (response, value) => {
    response.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
    response.end(JSON.stringify(value));
  };
  async function body(request) {
    let bytes = '';
    for await (const chunk of request) {
      bytes += chunk;
      assert(bytes.length <= 65536, 'Fixture request is too large');
    }
    return JSON.parse(bytes || '{}');
  }
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url, 'http://127.0.0.1');
      if (url.pathname === '/control') return json(response, control);
      if (url.pathname === '/snapshot') return json(response, { control, page: lastPage, events });
      if (url.pathname === '/configure' && request.method === 'POST') {
        const value = await body(request);
        if (value.phase) {
          assert(pictures[value.phase], 'Unknown fixture phase');
          control = { ...control, ...value, epoch: control.epoch + 1 };
        } else {
          control = { ...control, ...value };
        }
        return json(response, control);
      }
      if (url.pathname === '/event' && request.method === 'POST') {
        const value = await body(request);
        value.received_at = new Date().toISOString();
        events.push(value); lastPage = value;
        return json(response, { ok: true });
      }
      if (url.pathname.startsWith('/picture/')) {
        const phase = url.pathname.slice('/picture/'.length).replace(/\.png$/, '');
        assert(imageBytes.has(phase), 'Unknown fixture picture');
        response.writeHead(200, { 'Content-Type': 'image/png', 'Cache-Control': 'no-store' });
        return response.end(imageBytes.get(phase));
      }
      if (url.pathname === '/favicon.ico') {
        response.writeHead(204); return response.end();
      }
      response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' });
      response.end(`<!doctype html><html><head><meta charset="utf-8"><title>Recorded login fixture</title>
<style>html,body{margin:0;padding:0;width:1920px;height:1080px;overflow:hidden;background:#000}#screen{position:absolute;left:0;top:0;width:1920px;height:1080px;user-select:none;pointer-events:none}</style></head>
<body><img id="screen" draggable="false"><script>
(() => {
  const initial=${JSON.stringify(control)};
  const key='gamer-live-workflow-fixture';
  let cfg=initial;
  let data;
  try{data=JSON.parse(localStorage.getItem(key)||'null')}catch{}
  const fresh=()=>({epoch:cfg.epoch,phase:cfg.phase,cycle:0,reloads:0,cycleReloads:0,refreshes:0,logoutConfirmations:0,scanSwitches:0,loginDeadline:0,popup:false,popupArmed:false,popupShownInCycle:false,popupDismissals:0,popupBlockedInputs:0,popupAddClicks:0});
  if(!data||data.epoch!==cfg.epoch)data=fresh();
  data.reloads++;
  if(data.phase==='enter'||data.phase==='menu')data.cycleReloads++;
  const image=document.getElementById('screen');
  function report(reason,extra={}){
    localStorage.setItem(key,JSON.stringify(data));
    fetch('/event',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({...data,reason,at_ms:Date.now(),...extra}),keepalive:true}).catch(()=>{});
  }
  function show(phase,reason,extra={}){
    data.phase=phase; image.src='/picture/'+(data.popup?'desktop_popup':phase)+'.png'; report(reason,extra);
  }
  function showPopup(trigger){
    data.popup=true;data.popupArmed=false;data.popupShownInCycle=true;
    show(data.phase,'desktop_popup_shown',{trigger});
  }
  function blockPopupInput(action,pos){
    data.popupBlockedInputs++;report('desktop_popup_blocked_input',{action,...pos});
  }
  function scheduleLogin(){
    data.loginDeadline=cfg.autoLogin?Date.now()+cfg.loginDelayMs:0;
    localStorage.setItem(key,JSON.stringify(data));
  }
  // The short daily and enclosing script each reset once. Odd viewer cycles
  // show the real popup after the second reload; even cycles race the More
  // hover to prove that a visually matching background is not input-ready.
  if(cfg.desktopPopup&&data.cycle>0&&data.cycleReloads>=2&&!data.popupShownInCycle){
    if(data.cycle%2===1)showPopup('post_daily_reset');
    else{data.popupArmed=true;show(data.phase,'desktop_popup_armed');}
  }else show(data.phase,'load');
  if(data.phase==='qr'&&cfg.autoLogin&&!data.loginDeadline)scheduleLogin();
  const within=(event,x1,y1,x2,y2)=>event.clientX>=x1&&event.clientX<=x2&&event.clientY>=y1&&event.clientY<=y2;
  const requiresMoreClick=()=>cfg.requireMoreClick||(cfg.requireMoreClickOnEvenCycle&&data.cycle%2===0);
  document.addEventListener('mousemove',event=>{
    const pos={x:event.clientX,y:event.clientY};
    if(data.popup){blockPopupInput('hover',pos);return;}
    if(data.popupArmed&&data.phase==='enter'&&within(event,1804,10,1885,76)){
      showPopup('more_hover_race');blockPopupInput('hover',pos);return;
    }
    if(data.phase==='enter'&&within(event,1804,10,1885,76)){
      if(requiresMoreClick())report('more_hover_ignored',{...pos,requireMoreClick:true});
      else show('menu','more_hover',{...pos,requireMoreClick:false});
    }
  });
  document.addEventListener('click',event=>{
    const pos={x:event.clientX,y:event.clientY};
    if(data.popup){
      if(within(event,784,521,952,570)){
        data.popup=false;data.popupDismissals++;show(data.phase,'desktop_popup_later_click',pos);
      }else{
        if(within(event,968,521,1136,570))data.popupAddClicks++;
        blockPopupInput('click',pos);
      }
      return;
    }
    if(data.phase==='expired'&&within(event,930,503,990,563)){
      data.refreshes++;show('qr','expired_refresh',pos);scheduleLogin();
    }else if(data.phase==='enter'&&within(event,1804,10,1885,76)){
      show('menu','more_click',{...pos,requireMoreClick:requiresMoreClick()});
    }else if(data.phase==='menu'&&within(event,1550,262,1725,310)){
      show('confirm','logout_click',pos);
    }else if(data.phase==='confirm'&&within(event,970,507,1137,557)){
      data.logoutConfirmations++;data.loginDeadline=0;show('scan','logout_confirm',pos);
    }else if(data.phase==='scan'&&within(event,740,295,790,350)){
      data.scanSwitches++;data.cycle++;data.cycleReloads=0;data.popupShownInCycle=false;data.popupArmed=false;
      if(cfg.expireFirstScan&&data.scanSwitches===1){
        data.loginDeadline=0;show('expired','scan_switch',pos);
      }else{
        show('qr','scan_switch',pos);scheduleLogin();
      }
    }else{
      report('unhandled_click',pos);
    }
  });
  setInterval(async()=>{
    try{
      const next=await (await fetch('/control',{cache:'no-store'})).json();
      cfg=next;
      if(data.epoch!==cfg.epoch){data=fresh();show(data.phase,'configured');if(data.phase==='qr')scheduleLogin();}
      if(data.phase==='qr'&&cfg.autoLogin&&data.loginDeadline&&Date.now()>=data.loginDeadline){
        data.loginDeadline=0;show('enter','synthetic_login');
      }
    }catch(error){report('fixture_error',{error:String(error)})}
  },150);
})();
</script></body></html>`);
    } catch (error) {
      response.writeHead(400, { 'Content-Type': 'application/json' });
      response.end(JSON.stringify({ error: error.message }));
    }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  return {
    base, url: `${base}/fixture`,
    snapshot: () => ({ control, page: lastPage, events: [...events] }),
    configure: async values => {
      const result = await fetch(`${base}/configure`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(values) });
      assert(result.ok); return result.json();
    },
    close: async () => {
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
    },
  };
}
