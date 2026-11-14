import React, {useEffect,useRef,useState} from 'react';
import {createRoot} from 'react-dom/client';
import {invoke} from '@tauri-apps/api/core';
import {Power, VolumeX, ChevronRight, ChevronLeft, Play, Pause, Tv, Monitor, Info, MessageSquareText, X, RefreshCw} from 'lucide-react';
import './style.css';
type Media={title:string|null;artist:string|null;album:string|null;position:number|null;total_time:number|null;state:string};
type Context={connected:boolean;skip_backward:boolean;skip_forward:boolean;text_input:boolean;media:Media|null};
type Device={id:string;name:string;address:string};
function AppleTVBadge(){return <svg viewBox="0 0 34 24" aria-hidden="true"><path fill="currentColor" d="M10.7 5.9c1.7-.2 3-2 2.8-3.8-1.7.1-3.3 1.8-3.2 3.7l.4.1ZM14.9 8.2c-1-1.3-2.5-1.8-3.8-1.6-1.1.1-1.7.7-2.6.7-.9 0-1.6-.7-2.7-.7C3 6.6 1.3 9 1.5 11.9c.2 3.3 2 7.6 4.1 7.6 1 0 1.6-.6 2.8-.6 1.1 0 1.7.6 2.8.6 1.6-.1 3-2.7 3.6-4.3-2.6-1.2-3.1-4.8.1-7Z"/><path d="M20 5.5v11c0 2 1.1 2.5 2.5 1.8M17.5 9h5.5M25 9l3.1 9 3.1-9" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/></svg>}
function SkipIcon({forward=false}:{forward?:boolean}){
 return <svg viewBox="0 0 32 32" fill="none" aria-hidden="true"><g transform={forward?'translate(32 0) scale(-1 1)':undefined}><path d="M16 5a11 11 0 1 0 9.7 5.8" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/><path d="m19 1.8-3.5 3.3 3.5 3.2" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/></g><text x="16" y="18" textAnchor="middle" dominantBaseline="middle" fill="currentColor" fontFamily="-apple-system, sans-serif" fontSize="12" fontWeight="500">10</text></svg>
}
function App(){
 const [devices,setDevices]=useState<Device[]>([]),[device,setDevice]=useState<Device|null>(null),[panel,setPanel]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState(''),[pin,setPin]=useState(''),[pairing,setPairing]=useState(false),[connected,setConnected]=useState(false),[message,setMessage]=useState('');
 const [context,setContext]=useState<Context|null>(null),[mediaOpen,setMediaOpen]=useState(false);
 const [reconnecting,setReconnecting]=useState(true);
 const origin=useRef({x:0,y:0});
 async function call(action:string,args:Record<string,unknown>={}){setError('');try{return await invoke<any>('bridge',{request:{action,...args}})}catch(e){setError(String(e));throw e}}
 async function scan(){setBusy(true);try{const result=await call('scan');setDevices(result.devices)}catch{}finally{setBusy(false)}}
 async function choose(d:Device){setDevice(d);setConnected(false);setBusy(true);try{const r=await call('connect',{id:d.id});if(r.paired){setConnected(true);setPanel(false)}else{await call('pair_begin',{id:d.id});setPairing(true)}}catch{}finally{setBusy(false)}}
 async function finish(){setBusy(true);try{await call('pair_finish',{pin});await call('connect',{id:device?.id});setConnected(true);setPairing(false);setPanel(false);setPin('')}catch{}finally{setBusy(false)}}
 async function send(command:string){if(command==='text_prompt'){if(context?.text_input)setMessage('__prompt__');return}if(command==='info'){if(context?.media)setMediaOpen(true);return}if(!connected){setPanel(true);return}try{await call('command',{command})}catch{}}
 useEffect(()=>{
  let stopped=false;
  void invoke<{paired:boolean;device?:Device}>('bridge',{request:{action:'reconnect'}})
   .then(result=>{if(!stopped&&result.paired&&result.device){setDevice(result.device);setConnected(true)}})
   .catch(e=>{if(!stopped)setError(String(e))})
   .finally(()=>{if(!stopped)setReconnecting(false)});
  return()=>{stopped=true};
 },[]);
 useEffect(()=>{
  setContext(null);setMediaOpen(false);setMessage('');
  if(!connected)return;
  let stopped=false;
  let timer:ReturnType<typeof setTimeout>;
  async function refresh(){
   try{const next=await invoke<Context>('bridge',{request:{action:'context'}});if(!stopped){setContext(next);if(!next.connected)setConnected(false)}}
   catch{if(!stopped)setContext(null)}
   if(!stopped)timer=setTimeout(()=>void refresh(),1000);
  }
  void refresh();
  return()=>{stopped=true;clearTimeout(timer)};
 },[connected,device?.id]);
 useEffect(()=>{if(!context?.media)setMediaOpen(false);if(!context?.text_input)setMessage('')},[context]);
 useEffect(()=>{const key=(e:KeyboardEvent)=>{if((e.target as HTMLElement).tagName==='INPUT'||panel||message||mediaOpen)return;const map:Record<string,string>={ArrowUp:'up',ArrowDown:'down',ArrowLeft:'left',ArrowRight:'right',Enter:'select',Escape:'menu',' ':'play_pause',Backspace:'menu'};if(map[e.key]){e.preventDefault();void send(map[e.key])}};window.addEventListener('keydown',key);return()=>window.removeEventListener('keydown',key)},[connected,panel,message,mediaOpen]);
 const button=(icon:React.ReactNode,label:string,command:string,cls='')=><button className={`round ${cls}`} aria-label={label} title={label} onClick={()=>void send(command)}>{icon}</button>;
 return <main><div className="drag" data-tauri-drag-region/><section className="remote" aria-label="Apple TV remote"><header>{button(<VolumeX/>, 'Mute','mute')}<button className="device" disabled={reconnecting} onClick={()=>{setPanel(true);void scan()}}><span className="apple-tv"><AppleTVBadge/></span><span>{device?.name||'Apple TV'}</span><ChevronRight size={18}/></button>{button(<Power/>,'Sleep / wake','power')}</header><div className="touchpad"><div className="surface" role="button" tabIndex={0} aria-label="Touch surface: swipe to navigate or click to select" onPointerDown={e=>{origin.current={x:e.clientX,y:e.clientY};e.currentTarget.setPointerCapture(e.pointerId)}} onPointerUp={e=>{const dx=e.clientX-origin.current.x,dy=e.clientY-origin.current.y;void send(Math.max(Math.abs(dx),Math.abs(dy))<25?'select':Math.abs(dx)>Math.abs(dy)?dx>0?'right':'left':dy>0?'down':'up')}}/><div className="transport" aria-label="Available controls">{context?.skip_backward&&button(<SkipIcon/>,'Skip back 10 seconds','skip_backward','skip backward')}{context?.media&&button(<Info/>,'Media information','info','subtle')}{context?.text_input&&button(<MessageSquareText/>,'Send text','text_prompt')}{context?.skip_forward&&button(<SkipIcon forward/>,'Skip forward 10 seconds','skip_forward','skip forward')}</div></div><footer>{button(<span className="playpause"><Play fill="currentColor"/><Pause fill="currentColor"/></span>,'Play / pause','play_pause')}{button(<ChevronLeft/>,'Back','menu','back')}{button(<Monitor/>,'Home','home')}</footer><button className={`status ${connected?'online':''}`} disabled={reconnecting} onClick={()=>{setPanel(true);void scan()}}><i/>{reconnecting?'Reconnecting…':connected?'Connected': 'Select an Apple TV to connect'}</button></section>{error&&!panel&&<div className="toast" role="alert" onClick={()=>setError('')}>{error}</div>}{panel&&<div className="scrim"><section className="sheet"><button className="close" aria-label="Close" onClick={()=>setPanel(false)}><X/></button><Tv className="sheet-icon"/><h1>Connect to Apple TV</h1><p>Your computer and Apple TV must be on the same network.</p>{error&&<div className="error" role="alert">{error}</div>}{pairing?<><p>Enter the four-digit code shown on your TV.</p><input autoFocus inputMode="numeric" maxLength={4} placeholder="0000" value={pin} onChange={e=>setPin(e.target.value.replace(/\D/g,''))}/><button className="primary" disabled={busy||pin.length!==4} onClick={()=>void finish()}>Pair</button></>:<><div className="devices">{devices.map(d=><button key={d.id} disabled={busy} onClick={()=>void choose(d)}><Tv/><span>{d.name}<small>{d.address}</small></span><ChevronRight/></button>)}</div>{!devices.length&&<p>{busy?'Searching for Apple TVs…':'No devices found yet.'}</p>}<button className="primary" disabled={busy} onClick={()=>void scan()}><RefreshCw size={16} className={busy?'spin':''}/> Scan again</button></>}<p className="hint">Arrow keys · Enter to select · Esc to go back · Space to play</p></section></div>}{mediaOpen&&context?.media&&<div className="scrim"><section className="sheet"><button className="close" aria-label="Close" onClick={()=>setMediaOpen(false)}><X/></button><Info className="sheet-icon"/><h1>{context.media.title||'Now playing'}</h1>{context.media.artist&&<p>{context.media.artist}</p>}{context.media.album&&<p>{context.media.album}</p>}<p>{context.media.state}</p></section></div>}{message&&<div className="scrim"><section className="sheet"><h1>Send text</h1><input value={message==='__prompt__'?'':message} onChange={e=>setMessage(e.target.value)}/><button className="primary" onClick={()=>{void call("command",{command:"text",text:message==='__prompt__'?'':message}).then(()=>setMessage('')).catch(()=>{});}}>Send</button><button onClick={()=>setMessage('')}>Close</button></section></div>}</main>
}
createRoot(document.getElementById('root')!).render(<App/>);
