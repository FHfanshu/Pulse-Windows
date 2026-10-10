// Prerequisites: npm install --no-save --package-lock=false playwright; npx playwright install chromium
// Run: node scripts/check-card-motion.mjs [baseline|fixed]
// Optional: PULSE_MOTION_BASELINE_REF=v0.1.2; PULSE_PLAYWRIGHT_MODULE and PULSE_CHROMIUM_EXECUTABLE
// select an existing runtime. IPC uses fixtures only; the installed app and its settings are untouched.
import { createServer } from 'vite';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';

const playwrightModule = process.env.PULSE_PLAYWRIGHT_MODULE ?? 'playwright';
const playwrightSpecifier = playwrightModule.startsWith('file:')
  ? playwrightModule
  : isAbsolute(playwrightModule) || playwrightModule.startsWith('.')
    ? pathToFileURL(resolve(playwrightModule)).href
    : playwrightModule;
const { chromium } = await import(playwrightSpecifier);
const root = fileURLToPath(new URL('..', import.meta.url)).replaceAll('\\', '/').replace(/\/$/, '');
const baselineRef = process.env.PULSE_MOTION_BASELINE_REF || 'HEAD';
const baseline = execFileSync('git', ['show', `${baselineRef}:ui/src/panel/App.tsx`], { cwd: root, encoding: 'utf8' });
const mode = process.argv[2] ?? 'fixed';
const resultDir = mkdtempSync(join(tmpdir(), 'pulse-card-motion-'));
const videoDir = join(resultDir, 'video');
mkdirSync(videoDir, { recursive: true });
const fixture = `
import { mockIPC } from '/@fs/${root}/node_modules/@tauri-apps/api/mocks.js';
import { emit } from '/@fs/${root}/node_modules/@tauri-apps/api/event.js';
import { defaultMetrics, dockLayout } from '/src/panel/layout.ts';
const win = (i) => ({id:'limit'+i, kind:{type:i===0?'fiveHour':'weekly'}, scope:null,usedFraction:0.51,windowSeconds:18000,resetsAt:null,reportsLength:false,estimate:null,isExhausted:false,nextExpiry:null,label:'Test limit '+i});
const usage = (provider,count) => ({account:{provider,slot:''}, windows:Array.from({length:count},(_,i)=>win(i)), observedAt:null,state:{kind:'live'},plan:null,creditBalance:null,creditRemaining:null,origin:null,isCached:false});
const usages=[usage('claudeCode',1),usage('codex',6),usage('openCodeGo',2)];
const settings={language:'en',panelSize:'standard',railSpacing:'standard',detailedCards:[],splitAccounts:[],pinnedWindows:{},ringTints:{},enabledAccounts:['claudeCode','codex','openCodeGo'],usesGlass:false,sideRailShowsPercentages:true};
let edge='right', docked=true, geometry, layout, metrics={...defaultMetrics,railCapacity:3};
window.traceNativeShapes = (snapshot) => {window.latestShapes=snapshot;};
const updateLayout=async()=>{
 const vertical=edge==='right'||edge==='left';
 const g=geometry[(vertical?'vertical':'horizontal')+(docked?'Docked':'Free')];
 const rail={...g.rail,x:vertical?(edge==='right'?g.panel.w-g.rail.w:0):(g.panel.w-g.rail.w)/2,y:vertical?(g.panel.h-g.rail.h)/2:(edge==='bottom'?g.panel.h-g.rail.h:0)};
 layout={frame:{x:0,y:0,...g.panel},visible:{x:0,y:0,w:g.panel.w,h:g.panel.h},rail,edge,docked};
 await emit('panel-layout',layout);
};
mockIPC(async(cmd,args)=>{
 if(cmd==='get_settings')return {...settings};
 if(cmd==='get_snapshot')return {usages,refreshing:[]};
 if(cmd==='set_geometry'){geometry=args.geometry; await updateLayout();}
 if(cmd==='begin_backdrop_shapes')return 1;
 if(cmd==='get_card_spend_providers')return [];
 return null;
},{shouldMockEvents:true});
window.fixture={
 async hover(index){
   const D=dockLayout(metrics),vertical=edge==='left'||edge==='right';
   const along=D.firstRingAlong(docked,vertical?'vertical':'horizontal')+index*D.ringStep(vertical?'vertical':'horizontal',docked);
   const across=D.ringCentreAcross(vertical?'vertical':'horizontal',docked);
   await emit('pointer',{point:[layout.rail.x+(vertical?across:along),layout.rail.y+(vertical?along:across)],pressed:false,dragging:false});
 },
 async place(next,free=false){await emit('pointer',{point:null,pressed:false,dragging:false});edge=next;docked=!free; await updateLayout();},
 async scale(name){settings.panelSize=name;metrics.scale={small:0.82,standard:1,large:1.22}[name];await emit('settings-changed',{...settings});},
 async bounded(){layout.visible={x:0,y:100,w:layout.frame.w,h:230};await emit('panel-layout',layout);},
 sample(){
  const host=document.querySelector('.card-host'),svg=document.querySelector('.card-surface'),path=svg?.querySelector('path');
  if(!host||!svg||!path)return null;
  const nums=path.getAttribute('d').match(/-?\\d+(?:\\.\\d+)?/g).map(Number),vertical=edge==='right'||edge==='left';
  const tipAcross=edge==='right'?Number(svg.getAttribute('width')):edge==='left'?0:edge==='top'?0:Number(svg.getAttribute('height'));
  let tip;
  for(let i=0;i<nums.length;i+=2)if(Math.abs(nums[i+(vertical?0:1)]-tipAcross)<0.05){tip=nums[i+(vertical?1:0)];break;}
  const rect=svg.getBoundingClientRect(),s=rect.width/Number(svg.getAttribute('width'));
  const D=dockLayout(metrics),along0=(vertical?layout.rail.y:layout.rail.x)+D.firstRingAlong(docked,vertical?'vertical':'horizontal');
  return {tip:(vertical?rect.top:rect.left)+tip*s,along0,step:D.ringStep(vertical?'vertical':'horizontal',docked),top:rect.top,left:rect.left,height:rect.height,width:rect.width,scale:s,opacity:Number(getComputedStyle(host).opacity),native:window.latestShapes?.card};
 }
};
await import('/src/panel/main.tsx');
`;

const server = await createServer({
  configFile: root + '/vite.config.ts',
  server: { host: '127.0.0.1', port: 0, strictPort: false },
  plugins: [{ name: 'motion-fixture', enforce: 'pre',
    resolveId(id) { if(id==='/motion-fixture.js')return '\0motion-fixture.js'; },
    load(id) { if(id==='\0motion-fixture.js')return fixture; },
    transform(code,id) { if(mode==='baseline' && id.replaceAll('\\','/').endsWith('/src/panel/App.tsx')) return baseline; },
    configureServer(s) {s.middlewares.use(async(req,res,next)=>{
      if(req.url==='/motion-fixture.html'){res.setHeader('Content-Type','text/html');res.end(await s.transformIndexHtml(req.url,'<html><body style="background:#151b24"><div id="root"></div><script type="module" src="/motion-fixture.js"></script></body></html>'));}
      else next();
    });}
  }]
});
await server.listen();
const url = `http://127.0.0.1:${server.httpServer.address().port}/motion-fixture.html`;
const browser = await chromium.launch({ ...(process.env.PULSE_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PULSE_CHROMIUM_EXECUTABLE } : {}), headless: true });
const context = await browser.newContext({viewport:{width:1000,height:900},recordVideo:{dir:videoDir,size:{width:1000,height:900}}});
const page=await context.newPage();
const errors=[]; page.on('pageerror',e=>{errors.push(e.message);console.log('PAGE ERROR:',e.message);});
const results=[];
try{
 await page.goto(url);
 await page.waitForFunction(()=>document.querySelectorAll('.item').length===3);
 async function settle(i){await page.evaluate(i=>window.fixture.hover(i),i);await page.waitForTimeout(700);}
 async function sweep(from,to,duration=700){
  await settle(from);
  return page.evaluate(async({to,duration})=>{
   const frames=[window.fixture.sample()];await window.fixture.hover(to);
   const started=performance.now();
   await new Promise(resolve=>{function tick(){frames.push(window.fixture.sample());if(performance.now()-started<duration)requestAnimationFrame(tick);else resolve();}requestAnimationFrame(tick);});
   return frames.filter(Boolean);
  },{to,duration});
 }
 for(const edge of ['right','left','top','bottom']){
  await page.evaluate(edge=>window.fixture.place(edge),edge);await page.waitForTimeout(750);
  for(const [from,to] of [[1,2],[2,1]]){
   const frames=await sweep(from,to);
   const start=frames[0].tip,end=frames.at(-1).tip,dir=Math.sign(end-start);
   const jump=Math.abs(frames[1].tip-start);
   const backwards=Math.max(0,...frames.slice(1).map((f,i)=>-(f.tip-frames[i].tip)*dir));
   const escape=Math.max(0,...frames.map(f=>Math.max(Math.min(start,end)-f.tip,f.tip-Math.max(start,end))));
   const nativeError=Math.max(...frames.filter(f=>f.native).map(f=>Math.abs((edge==='right'||edge==='left'?f.native.y:f.native.x)-(edge==='right'||edge==='left'?f.top:f.left))));
   results.push({edge,from,to,jump,backwards,escape,nativeError,frames});
   if(mode!=='baseline'){
    assert.ok(jump<20,edge+' pointer must not jump on selection: '+jump);
    assert.ok(backwards<2,edge+' pointer must not reverse away from target: '+backwards);
    assert.ok(escape<2,edge+' pointer must not leave the travel interval: '+escape);
    assert.ok(Math.abs(end-(frames.at(-1).along0+to*frames.at(-1).step))<1,edge+' must settle on selected ring');
   }
  }
 }
 // Interrupt movement before it has settled: a sweep must continue from its painted position.
 await page.evaluate(()=>window.fixture.place('right'));await page.waitForTimeout(700);await settle(1);
 const rapid=await page.evaluate(async()=>{
  const frames=[];let next=2;let start=performance.now(),last=start;
  await window.fixture.hover(next);
  await new Promise(resolve=>{async function tick(){frames.push(window.fixture.sample());const now=performance.now();if(now-last>75&&now-start<650){next=next===1?2:1;last=now;await window.fixture.hover(next);}if(now-start<1500)requestAnimationFrame(tick);else resolve();}requestAnimationFrame(tick);});return frames.filter(Boolean);
 });
 const maxStep=Math.max(...rapid.slice(1).map((f,i)=>Math.abs(f.tip-rapid[i].tip)));
 results.push({rapidMaxStep:maxStep,frames:rapid});
 if(mode!=='baseline')assert.ok(maxStep<35,'rapid interruption must not snap: '+maxStep);
 for(const [name,scale] of [['small',0.82],['standard',1],['large',1.22]]){
  await page.evaluate(name=>window.fixture.scale(name),name);await page.waitForTimeout(750);await settle(2);
  const sample=await page.evaluate(()=>window.fixture.sample());
  assert.ok(Math.abs(sample.width-270*scale)<1,name+' dimensions must match scale');
  assert.ok(Math.abs(sample.tip-(sample.along0+2*sample.step))<1,name+' pointer must align with selected ring');
 }
 await page.evaluate(()=>window.fixture.scale('standard'));await page.waitForTimeout(750);
 await page.evaluate(()=>window.fixture.place('right',true));await page.waitForTimeout(700);
 await settle(1);const freeFrames=await sweep(1,2);results.push({free:true,frames:freeFrames});
 if(mode!=='baseline')assert.ok(Math.abs(freeFrames.at(-1).tip-(freeFrames.at(-1).along0+2*freeFrames.at(-1).step))<1,'floating pointer settles on ring');
 await page.evaluate(()=>window.fixture.place('right'));await page.waitForTimeout(700);
 await page.evaluate(()=>window.fixture.bounded());await settle(1);
 const bounded=await page.evaluate(()=>({sample:window.fixture.sample(),scrolls:!!document.querySelector('.card-scroll.scrolls')}));
 assert.ok(bounded.scrolls,'tall card scrolls when screen room is limited');
 assert.ok(bounded.sample.top>=99&&bounded.sample.top+bounded.sample.height<=331,'bounded card stays on screen');
 results.push({bounded});
 await page.screenshot({path:join(resultDir,`pulse-motion-${mode}.png`)});
 assert.deepEqual(errors,[],'no browser exceptions');
 console.log(JSON.stringify(results.map(({frames,...r})=>r),null,2));
}finally{
 writeFileSync(join(resultDir,`pulse-motion-${mode}-trace.json`),JSON.stringify({results,errors},null,2));
 console.log(`Motion evidence: ${resultDir}`);
 await context.close();await browser.close();await server.close();
}
