(async () => {
  const complete = element => new Promise(resolve => {element.onload=element.onerror=()=>resolve();document.body.appendChild(element)});
  const image=document.createElement('img'); image.src='http://nav.persona.test/image';
  const script=document.createElement('script'); script.src='http://nav.persona.test/script';
  const xhr=new Promise(resolve=>{const x=new XMLHttpRequest();x.onloadend=resolve;x.open('GET','/xhr');x.send()});
  const ws=new Promise(resolve=>{const w=new WebSocket('ws://nav.persona.test/websocket');w.onerror=w.onclose=resolve});
  const tls=Array.from({length:12},(_,i)=>fetch(`https://tls${i}.persona.test/`).catch(()=>null));
  await Promise.all([complete(image),complete(script),xhr,ws,fetch('/fetch'),fetch('http://cross.persona.test/cors',{method:'POST',headers:{'X-Persona':'test'},body:'body'}).catch(()=>null),...tls]);
  return 'done';
})()
