(async () => {
  const outcome = async f => { try { let v = await Promise.race([Promise.resolve().then(f), new Promise(resolve => setTimeout(() => resolve('pending after 2 seconds'), 2000))]); return {result: v === undefined ? 'undefined' : v}; } catch (e) { return {error: {name: e.name, message: e.message}}; } };
  const inventory = root => {
    const globals = {}, prototypes = {};
    for (const name of Object.getOwnPropertyNames(root).sort()) {
      let value;
      try { value = root[name]; globals[name] = typeof value; } catch (e) { globals[name] = 'throws:' + e.name; continue; }
      if (typeof value === 'function' && value.prototype) {
        const chain = []; let p = value.prototype;
        while (p) { chain.push(Object.getOwnPropertyNames(p).sort()); p = Object.getPrototypeOf(p); }
        prototypes[name] = chain;
      }
    }
    return {globals, prototypes};
  };
  const gl = {};
  for (const kind of ['webgl', 'webgl2']) {
    const c = document.createElement('canvas').getContext(kind);
    if (!c) { gl[kind] = null; continue; }
    const debug = c.getExtension('WEBGL_debug_renderer_info');
    const parameters = {};
    for (const key of ['VERSION','SHADING_LANGUAGE_VERSION','VENDOR','RENDERER','MAX_TEXTURE_SIZE','MAX_CUBE_MAP_TEXTURE_SIZE','MAX_RENDERBUFFER_SIZE','MAX_VIEWPORT_DIMS','MAX_VERTEX_ATTRIBS','MAX_VERTEX_UNIFORM_VECTORS','MAX_FRAGMENT_UNIFORM_VECTORS','MAX_VARYING_VECTORS','MAX_TEXTURE_IMAGE_UNITS','MAX_VERTEX_TEXTURE_IMAGE_UNITS','MAX_COMBINED_TEXTURE_IMAGE_UNITS','MAX_SAMPLES','MAX_3D_TEXTURE_SIZE','MAX_ARRAY_TEXTURE_LAYERS','MAX_DRAW_BUFFERS','MAX_COLOR_ATTACHMENTS']) {
      if (c[key] !== undefined) { const v = c.getParameter(c[key]); parameters[key] = ArrayBuffer.isView(v) ? Array.from(v) : v; }
    }
    gl[kind] = {parameters, extensions: (c.getSupportedExtensions() || []).sort(), unmasked_vendor: debug ? c.getParameter(debug.UNMASKED_VENDOR_WEBGL) : null, unmasked_renderer: debug ? c.getParameter(debug.UNMASKED_RENDERER_WEBGL) : null};
  }
  gl.webgpu = {presence: typeof navigator.gpu, adapter: navigator.gpu ? await outcome(async () => (await navigator.gpu.requestAdapter()) ? 'adapter' : null) : 'unavailable'};
  const types = ['video/mp4; codecs="avc1.42E01E"','video/mp4; codecs="avc1.4D401E"','video/mp4; codecs="avc1.640028"','video/mp4; codecs="hvc1.1.6.L93.B0"','video/mp4; codecs="hev1.1.6.L93.B0"','video/mp4; codecs="av01.0.04M.08"','video/webm; codecs="vp8,opus"','video/webm; codecs="vp9,opus"','video/webm; codecs="vp09.00.10.08"','video/webm; codecs="vp09.02.10.10"','video/webm; codecs="av01.0.04M.08"','video/ogg; codecs="theora,vorbis"','audio/mp4; codecs="mp4a.40.2"','audio/webm; codecs="opus"','audio/webm; codecs="vorbis"','audio/ogg; codecs="opus"','audio/ogg; codecs="vorbis"','audio/wav; codecs="1"','audio/mpeg','audio/flac','video/mp2t; codecs="avc1.42E01E,mp4a.40.2"'];
  const media = {};
  for (const t of types) media[t] = {canPlayType: document.createElement('video').canPlayType(t), mediaSource: typeof MediaSource === 'undefined' ? 'undefined' : MediaSource.isTypeSupported(t), mediaRecorder: typeof MediaRecorder === 'undefined' ? 'undefined' : MediaRecorder.isTypeSupported(t)};
  const codecs = {};
  const video = ['avc1.42E01E','avc1.4D401E','avc1.640028','hvc1.1.6.L93.B0','hev1.1.6.L93.B0','vp8','vp09.00.10.08','vp09.02.10.10','av01.0.04M.08'];
  const audio = ['opus','vorbis','mp4a.40.2','mp3','flac','pcm-u8','pcm-s16','theora'];
  for (const [name, list] of [['VideoDecoder',video],['VideoEncoder',video],['AudioDecoder',audio],['AudioEncoder',audio]]) {
    codecs[name] = {};
    for (const codec of list) {
      const config = name.startsWith('Video') ? {codec, codedWidth: 64, codedHeight: 64, width: 64, height: 64, bitrate: 100000, framerate: 30} : {codec, sampleRate: 48000, numberOfChannels: 2, bitrate: 128000};
      codecs[name][codec] = typeof globalThis[name] === 'undefined' ? 'undefined' : await outcome(async () => (await globalThis[name].isConfigSupported(config)).supported);
    }
  }
  const images = {};
  const imageFixtures = __IMAGE_FIXTURES__;
  for (const type of ['image/png','image/jpeg','image/webp','image/gif','image/avif','image/jxl','image/bmp','image/x-icon','image/svg+xml']) {
    images[type] = {imageDecoder: typeof ImageDecoder === 'undefined' ? 'undefined' : await outcome(() => ImageDecoder.isTypeSupported(type)), createImageBitmap: !imageFixtures[type] ? 'no-fixture' : await outcome(async () => { const bytes = Uint8Array.from(atob(imageFixtures[type]), c => c.charCodeAt(0)); const bitmap = await createImageBitmap(new Blob([bytes], {type})); const size = [bitmap.width, bitmap.height]; bitmap.close(); return size; })};
  }
  let xslt = {presence: typeof XSLTProcessor};
  if (typeof XSLTProcessor !== 'undefined') xslt.transform = await outcome(() => { const p = new DOMParser(); const x = p.parseFromString('<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:template match="/"><out>ok</out></xsl:template></xsl:stylesheet>', 'application/xml'); const t = new XSLTProcessor(); t.importStylesheet(x); return new XMLSerializer().serializeToString(t.transformToDocument(p.parseFromString('<in/>','application/xml'))); });
  const notifications = {chrome: typeof chrome, chromeKeys: typeof chrome === 'undefined' ? [] : Object.keys(chrome).sort(), chromeNotifications: typeof chrome === 'undefined' ? 'undefined' : typeof chrome.notifications, notification: typeof Notification, permission: typeof Notification === 'undefined' ? 'undefined' : Notification.permission};
  const rtc = {peerConnection: typeof RTCPeerConnection, mediaDevices: typeof navigator.mediaDevices};
  rtc.offer = typeof RTCPeerConnection === 'undefined' ? 'undefined' : await outcome(async () => { const pc = new RTCPeerConnection({iceServers: []}); try { pc.createDataChannel('probe'); const offer = await pc.createOffer(); return {type: offer.type, sdpType: typeof offer.sdp}; } finally { pc.close(); } });
  for (const method of ['enumerateDevices','getUserMedia','getDisplayMedia']) rtc[method] = !navigator.mediaDevices || !navigator.mediaDevices[method] ? 'undefined' : await outcome(async () => { const v = await navigator.mediaDevices[method](method === 'enumerateDevices' ? undefined : {audio:true,video:true}); if (Array.isArray(v)) return {length: v.length, devices: v.map(d => ({kind:d.kind, keys:Object.keys(d.toJSON()).sort()}))}; v.getTracks().forEach(t => t.stop()); return 'stream'; });
  const privacy = {};
  for (const [name, object, key, args] of [['browsingTopics',document,'browsingTopics',[]],['joinAdInterestGroup',navigator,'joinAdInterestGroup',[{}]],['runAdAuction',navigator,'runAdAuction',[{}]],['sharedStorage',globalThis,'sharedStorage',[]],['privateAggregation',globalThis,'privateAggregation',[]],['hasPrivateToken',document,'hasPrivateToken',['https://example.com']],['hasRedemptionRecord',document,'hasRedemptionRecord',['https://example.com']]]) {
    let value; try { value = object[key]; } catch (e) { privacy[name] = {error:{name:e.name,message:e.message}}; continue; }
    privacy[name] = {type: typeof value}; if (typeof value === 'function') privacy[name].call = await outcome(() => value.apply(object,args));
  }
  for (const key of ['HTMLFencedFrameElement','AttributionReporting','AttributionReportingRequestOptions']) privacy[key] = typeof globalThis[key];
  privacy.attributionSrc = typeof document.createElement('a').attributionSrc;
  const workerSource = `(${inventory.toString()})(globalThis)`;
  const worker = await outcome(() => new Promise((resolve,reject) => { const blob = new Blob(['postMessage(' + workerSource + ')'], {type:'text/javascript'}); const u = URL.createObjectURL(blob); const w = new Worker(u); w.onmessage = e => { resolve(e.data); w.terminate(); URL.revokeObjectURL(u); }; w.onerror = e => { reject(new Error(e.message)); w.terminate(); URL.revokeObjectURL(u); }; }));
  return {webgl:gl, codecs:{media,webcodecs:codecs,images}, xslt, notifications, webrtc:rtc, privacy, inventory:{window:inventory(globalThis), dedicatedWorker:worker, serviceWorker: null, speculationRules: HTMLScriptElement.supports('speculationrules'), prerendering:typeof document.prerendering === 'undefined' ? 'undefined' : document.prerendering, modelContext:typeof document.modelContext}};
})()
