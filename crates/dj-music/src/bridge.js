// Daijobu Island bridge. Injected into every document the YouTube Music engine loads,
// but it does nothing unless the page is music.youtube.com. It only reads the page and
// posts JSON through window.chrome.webview; the controls at the bottom run on request.
(() => {
  'use strict';
  if (location.hostname !== 'music.youtube.com' || window.__djBridge) return;
  window.__djBridge = true;

  // Page selectors. YouTube Music changes its DOM, so every selector lives here.
  const SEL = {
    avatar: '#avatar-btn',
    signIn: 'a[href*="accounts.google.com/ServiceLogin"], a[href*="accounts.google.com/signin"]',
    playPause: '#play-pause-button',
    next: '.next-button',
    prev: '.previous-button',
  };
  const TICK_MS = 1000; // state and auth check; posts only on change
  const VU_MS = 34; // about 30 Hz while playing
  const VU_BANDS = 22;
  const VU_SILENT_LIMIT = 240; // about 8 s of all-zero analyser output: give up

  const post = (obj) => {
    try {
      window.chrome.webview.postMessage(obj);
    } catch (_) {
      /* not hosted in WebView2 */
    }
  };
  const media = () => document.querySelector('video, audio');
  const finiteOr0 = (v) => (Number.isFinite(v) && v > 0 ? v : 0);

  // Largest artwork by its "WxH" sizes string.
  function bestArt(meta) {
    let best = '';
    let bestArea = -1;
    for (const a of (meta && meta.artwork) || []) {
      const m = /(\d+)x(\d+)/.exec(a.sizes || '');
      const area = m ? Number(m[1]) * Number(m[2]) : 0;
      if (a.src && area >= bestArea) {
        best = a.src;
        bestArea = area;
      }
    }
    return best;
  }

  function snapshot() {
    const m = media();
    const meta = navigator.mediaSession && navigator.mediaSession.metadata;
    if (!m || !meta || !meta.title) {
      return { t: 'state', title: '', artist: '', album: '', art: '', playing: false, pos: 0, dur: 0 };
    }
    return {
      t: 'state',
      title: meta.title,
      artist: meta.artist || '',
      album: meta.album || '',
      art: bestArt(meta),
      playing: !m.paused && !m.ended,
      pos: Math.floor(finiteOr0(m.currentTime)),
      dur: Math.floor(finiteOr0(m.duration)),
    };
  }

  let lastState = '';
  function reportState() {
    const s = snapshot();
    const key = JSON.stringify(s);
    if (key !== lastState) {
      lastState = key;
      post(s);
    }
  }

  let lastAuth = null;
  function reportAuth() {
    const signedIn = !!document.querySelector(SEL.avatar);
    const signedOut = !signedIn && !!document.querySelector(SEL.signIn);
    if (!signedIn && !signedOut) return; // page still loading: decide later
    if (signedIn !== lastAuth) {
      lastAuth = signedIn;
      post({ t: 'auth', signedIn });
    }
  }

  // ---- VU: the page's own audio, through an analyser that feeds the speakers. ----
  let ctx = null;
  let analyser = null;
  let freq = null;
  let srcEl = null;
  let vuTimer = 0;
  let vuDead = false;
  let silent = 0;
  // Log-spaced bin edges over the 512 usable bins of a 1024-point FFT.
  const edges = [];
  for (let b = 0; b <= VU_BANDS; b++) edges.push(Math.floor(Math.pow(511, b / VU_BANDS)));

  function giveUpVu() {
    if (!vuDead) {
      vuDead = true;
      post({ t: 'bands', v: null });
    }
    stopVu();
  }

  function attachAudio(m) {
    if (vuDead || srcEl === m) return;
    try {
      ctx = ctx || new (window.AudioContext || window.webkitAudioContext)();
      const source = ctx.createMediaElementSource(m);
      analyser = ctx.createAnalyser();
      analyser.fftSize = 1024;
      source.connect(analyser);
      analyser.connect(ctx.destination);
      freq = new Uint8Array(analyser.frequencyBinCount);
      srcEl = m;
    } catch (_) {
      giveUpVu();
    }
  }

  function sampleBands() {
    analyser.getByteFrequencyData(freq);
    const v = [];
    let sum = 0;
    for (let b = 0; b < VU_BANDS; b++) {
      const s = edges[b];
      const e = Math.max(s + 1, edges[b + 1]);
      let acc = 0;
      for (let i = s; i < e && i < freq.length; i++) acc += freq[i];
      const level = Number((acc / (e - s) / 255).toFixed(3));
      v.push(level);
      sum += level;
    }
    return { v, sum };
  }

  function vuTick() {
    const m = media();
    if (!m || m.paused) return stopVu();
    attachAudio(m);
    if (vuDead || !analyser || srcEl !== m) return;
    if (ctx.state === 'suspended') ctx.resume();
    const { v, sum } = sampleBands();
    silent = sum === 0 ? silent + 1 : 0;
    if (silent >= VU_SILENT_LIMIT) return giveUpVu();
    post({ t: 'bands', v });
  }

  function startVu() {
    if (!vuDead && !vuTimer) vuTimer = setInterval(vuTick, VU_MS);
  }

  function stopVu() {
    if (vuTimer) {
      clearInterval(vuTimer);
      vuTimer = 0;
    }
  }

  const onMedia = (e) => {
    if (e.type === 'play') {
      attachAudio(media());
      if (ctx && ctx.state === 'suspended') ctx.resume();
      startVu();
    } else if (e.type === 'pause' || e.type === 'ended') {
      stopVu();
    }
    reportState();
  };
  for (const ev of ['play', 'pause', 'ended', 'loadedmetadata', 'seeked']) {
    document.addEventListener(ev, onMedia, true);
  }

  setInterval(() => {
    reportState();
    reportAuth();
  }, TICK_MS);

  // ---- Controls, called from Rust with ExecuteScript. ----
  const clickOr = (sel, fallback) => {
    const button = document.querySelector(sel);
    if (button) button.click();
    else fallback();
  };
  window.__dj = {
    toggle() {
      const m = media();
      clickOr(SEL.playPause, () => {
        if (m) (m.paused ? m.play() : m.pause());
      });
    },
    next() {
      clickOr(SEL.next, () => {});
    },
    prev() {
      clickOr(SEL.prev, () => {});
    },
    seek(sec) {
      const m = media();
      if (m && Number.isFinite(sec)) m.currentTime = Math.max(0, sec);
    },
  };
})();
