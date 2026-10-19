// Isolated Vue surface acceptance; no connection to account/device APIs.
const {chromium}=require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const path=require('node:path');
const fs=require('node:fs');
const {spawn}=require('node:child_process');
const net=require('node:net');
(async()=>{
  const root=path.resolve(__dirname,'..');
  const server=net.createServer();await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));const port=server.address().port;await new Promise(resolve=>server.close(resolve));
  const fixture=path.join(root,'web',`.runtime-qa-${process.pid}.html`);
  fs.writeFileSync(fixture,`<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="app" style="max-width:1100px;margin:24px auto;padding:0 12px"></div><script type="module">import {createApp} from 'vue';import Component from '/src/components/RuntimeDependencies.vue';import '/src/styles/theme.css';createApp(Component).mount('#app')</script></html>`,{flag:'wx'});
  const vite=spawn(process.execPath,[path.join(root,'web/node_modules/vite/bin/vite.js'),'--host','127.0.0.1','--port',String(port),'--strictPort'],{cwd:path.join(root,'web'),stdio:'ignore',windowsHide:true});
  let browser;
  try {
    const url=`http://127.0.0.1:${port}/${path.basename(fixture)}`;
    for(let attempt=0;;attempt++){try{if((await fetch(url)).ok)break;}catch{}if(attempt>80)throw new Error('Isolated Vite did not start');await new Promise(resolve=>setTimeout(resolve,250));}
    browser=await chromium.launch({...(process.platform==='win32'?{channel:'msedge'}:{}),headless:true});
    const page=await browser.newPage({viewport:{width:1280,height:900}});
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    const deps=['adb','ffmpeg','browser','tesseract','tessdata','scrcpy-server'].map(id=>({id,status:id==='tesseract'?'missing':'ready',source:id==='scrcpy-server'?'application':'system',version:id==='tesseract'?null:'本机版本',path:id==='tesseract'?null:'C:/existing/tools/'+id,configured_path:'',detail:'未找到可用依赖',auto_install:id!=='scrcpy-server',restart_required:false}));
    await page.route('**/api/system/dependencies**',route=>route.fulfill({status:route.request().method()==='PUT'?409:200,contentType:'application/json',body:JSON.stringify(route.request().method()==='PUT'?{error:'配置已变化，请重新读取'}:{revision:'r1',dependencies:deps})}));
    await page.goto(url);
    await page.getByText('文字识别（Tesseract）',{exact:true}).waitFor();
    const ocr=page.locator('.runtime-list>li').filter({hasText:'文字识别（Tesseract）'});
    await ocr.locator('summary').click();
    await page.screenshot({path:path.resolve('server/target/runtime-ui-desktop.png'),fullPage:true});
    await page.setViewportSize({width:390,height:844});
    await ocr.locator('input').fill('D:/user/tesseract.exe');await ocr.getByText('保存选择',{exact:true}).click();
    await page.getByRole('alert').waitFor();
    if(await ocr.locator('input').inputValue()!=='D:/user/tesseract.exe')throw new Error('Conflict lost user input');
    if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth))throw new Error('Mobile horizontal overflow');
    await page.screenshot({path:path.resolve('server/target/runtime-ui-mobile.png'),fullPage:true});
    if(errors.length)throw new Error(errors.join('\n'));
    console.log(JSON.stringify({desktop:'1280x900',mobile:'390x844',horizontal_overflow:false,conflict_preserves_input:true,page_errors:errors}));
  } finally {if(browser)await browser.close();vite.kill();fs.unlinkSync(fixture);}
})().catch(e=>{console.error(e.message);process.exitCode=1});
