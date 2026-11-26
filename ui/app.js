// 817 Monitor UI. Talks to the Rust back end through Tauri; when opened in a
// plain browser (UI development), it falls back to a small built-in mock.

(function () {
  const $ = (id) => document.getElementById(id);
  const TAURI = window.__TAURI__;
  const invoke = TAURI ? TAURI.core.invoke : mockInvoke;
  const listen = TAURI ? TAURI.event.listen : mockListen;

  // ------------------------------------------------------------ settings
  const DEFAULTS = {
    kind: "serial", port: "", baud: "38400", host: "127.0.0.1", tcpPort: 4532,
    readBd: false, meterMap: "tested", theme: "radio", onTop: false, autoConnect: false,
  };
  let cfg = { ...DEFAULTS };
  try { cfg = { ...DEFAULTS, ...JSON.parse(localStorage.getItem("ft817mon") || "{}") }; } catch (_) {}
  // v2: the field-tested meter layout became the default (Nov 2026).
  if (cfg.meterMapV !== 2) { cfg.meterMap = "tested"; cfg.meterMapV = 2; }
  const save = () => { try { localStorage.setItem("ft817mon", JSON.stringify(cfg)); } catch (_) {} };

  // ------------------------------------------------------------ meters
  function buildBar(el) {
    el.innerHTML = "";
    for (let i = 1; i <= 15; i++) {
      const s = document.createElement("i");
      if (el.id === "sbar" && i > 9) s.dataset.over = "1";
      el.appendChild(s);
    }
  }
  ["sbar", "po", "swr", "alc", "mod"].forEach((id) => buildBar($(id)));

  function setBar(id, value, peak) {
    const el = $(id);
    el.classList.toggle("unavailable", value == null);
    [...el.children].forEach((seg, i) => {
      const n = i + 1;
      const on = value != null && n <= value;
      seg.className = on ? "on" : (peak != null && n === peak ? "peak" : "");
      if (on && seg.dataset.over) seg.classList.add("over");
    });
  }

  // S-meter peak hold: hold 1.5 s, then fall one step every 120 ms
  let peak = 0, peakAt = 0;
  function sPeak(v) {
    const now = performance.now();
    if (v >= peak) { peak = v; peakAt = now; }
    else if (now - peakAt > 1500) { peak = Math.max(v, peak - 1); peakAt = now - 1380; }
    return peak;
  }

  // ------------------------------------------------------------ flags row
  const FLAGS = [
    ["agc", (s) => s.agc == null ? null : { text: "AGC-" + s.agc[0], on: s.agc !== "OFF" }],
    ["nb", (s) => b(s.nb, "NB")],
    ["dsp", (s) => b(s.dsp, "DSP")],
    ["pbt", (s) => b(s.pbt, "PBT")],
    ["lock", (s) => b(s.lock, "LOCK")],
    ["fst", (s) => b(s.fst, "FST")],
    ["split", (s) => b(s.split, "SPLIT")],
    ["vox", (s) => b(s.vox, "VOX")],
    ["bk", (s) => b(s.bk, "BK")],
    ["kyr", (s) => b(s.kyr, "KYR")],
    ["ant", (s) => s.antenna == null ? null : { text: "ANT-" + s.antenna[0], on: s.antenna === "REAR" }],
    ["pwr", (s) => s.tx_power == null ? null : { text: s.tx_power, on: true }],
    ["tone", (s) => s.tone_mismatch ? { text: "TONE?", on: true, alert: true } : { text: "TONE?", on: false, alert: true }],
    ["disc", (s) => s.discr_off ? { text: "OFF-FREQ", on: true, alert: true } : { text: "OFF-FREQ", on: false, alert: true }],
    ["hswr", (s) => s.high_swr ? { text: "HI-SWR", on: true, alert: true } : { text: "HI-SWR", on: false, alert: true }],
  ];
  function b(v, text) { return v == null ? null : { text, on: v }; }
  const flagEls = {};
  for (const [key, fn] of FLAGS) {
    const sp = document.createElement("span");
    sp.textContent = fn({})?.text ?? key.toUpperCase();
    $("flags").appendChild(sp);
    flagEls[key] = sp;
  }

  // ------------------------------------------------------------ theme
  let radioColor = null;
  function applyTheme() {
    const t = cfg.theme === "radio" ? (radioColor || "amber") : cfg.theme;
    document.body.className = "theme-" + t;
  }

  // ------------------------------------------------------------ render
  let last = null;
  let running = false;
  function render(s) {
    last = s;
    // status bar
    const dot = $("dot");
    dot.className = "dot " + s.link;
    $("link-text").textContent = s.link === "lost" && s.retry_in ? `No link · retrying in ${s.retry_in} s` : { up: "Connected", connecting: "Connecting…", lost: "No link", stopped: "Stopped" }[s.link];
    $("source").textContent = s.source || "Not connected";
    $("rate").textContent = s.link === "up" ? `${Math.round(s.polls_per_sec)} polls/s` : "";
    running = s.link !== "stopped";
    $("connect-btn").textContent = running ? "Disconnect" : "Connect";
    $("connect-btn").classList.toggle("danger", running);

    // no-link overlay
    const down = s.link !== "up";
    $("lcd").classList.toggle("dim", down);
    $("nolink").classList.toggle("hidden", !down);
    // While retrying after an error, keep saying NO LINK instead of flashing CONNECTING.
    $("nolink-title").textContent = s.link === "connecting" && s.error ? "NO LINK" : ({ connecting: "CONNECTING", lost: "NO LINK", stopped: "STANDBY" }[s.link] || "");
    // The error stays on screen through retries; attempts and the countdown
    // to the next one are shown on their own line underneath.
    if (s.link === "stopped") {
      $("nolink-msg").textContent = "Press Connect, or open Settings (⚙) to choose the port.";
      $("nolink-status").textContent = "";
    } else {
      $("nolink-msg").textContent = s.error || "";
      $("nolink-status").textContent = s.message || "";
    }
    if (down) return;

    if (s.lcd_color && s.lcd_color !== radioColor) { radioColor = s.lcd_color; applyTheme(); }

    renderFrequency($("freq"), s.freq_hz);
    $("vfo").textContent = s.vfo ?? "VFO ?";
    $("band").textContent = s.band ?? "—";
    $("mode").textContent = s.mode ?? "—";

    const txrx = $("txrx");
    txrx.textContent = s.tx ? "TX" : "RX";
    txrx.classList.toggle("tx", s.tx);
    $("busy").classList.toggle("off", s.tx || s.squelched !== false);

    $("rx-meter").classList.toggle("hidden", s.tx);
    $("tx-meter").classList.toggle("hidden", !s.tx);
    if (s.tx) {
      setBar("po", s.po);
      $("poval").textContent = s.po ?? "—";
      setBar("swr", s.swr); $("swrval").textContent = s.swr ?? "—";
      setBar("alc", s.alc); $("alcval").textContent = s.alc ?? "—";
      setBar("mod", s.modulation); $("modval").textContent = s.modulation ?? "—";
    } else {
      const v = s.s_raw ?? 0;
      setBar("sbar", s.s_raw, sPeak(v));
      $("sval").textContent = s.s_label ?? "—";
    }

    for (const [key, fn] of FLAGS) {
      const r = fn(s);
      const el = flagEls[key];
      if (r == null) { el.className = "na"; continue; }
      el.textContent = r.text;
      el.className = r.alert ? (r.on ? "alert" : "gone") : (r.on ? "on" : "off");
    }
  }

  // ------------------------------------------------------------ connect
  function monitorConfig() {
    const source = cfg.kind === "serial"
      ? { kind: "serial", port: cfg.port, baud: Number(cfg.baud) }
      : cfg.kind === "rigctld"
        ? { kind: "rigctld", host: cfg.host, port: Number(cfg.tcpPort) }
        : { kind: "demo" };
    return { source, read_tx_meters: cfg.readBd, meter_map: cfg.meterMap };
  }

  async function connect() {
    if (cfg.kind === "serial" && !cfg.port) {
      openSettings();
      return;
    }
    cfg.autoConnect = true; save();
    try { await invoke("start_monitor", { config: monitorConfig() }); }
    catch (e) { render({ link: "lost", message: String(e), source: "" }); }
  }
  async function disconnect() {
    cfg.autoConnect = false; save();
    await invoke("stop_monitor");
  }
  $("connect-btn").addEventListener("click", () => (running ? disconnect() : connect()));

  // ------------------------------------------------------------ settings window
  // The settings live here; the separate Settings window asks for them,
  // edits them and sends each change back.
  const openSettings = () => invoke("open_settings").catch(() => {});
  $("settings-btn").addEventListener("click", openSettings);
  let pendingReconnect = false;
  listen("settings-request", () => {
    if (TAURI) TAURI.event.emit("settings-config", cfg);
  });
  listen("settings-changed", (e) => {
    const { cfg: next, connection } = e.payload;
    const onTopChanged = next.onTop !== cfg.onTop;
    cfg = { ...cfg, ...next, autoConnect: cfg.autoConnect };
    save();
    applyTheme();
    if (onTopChanged && TAURI) TAURI.window.getCurrentWindow().setAlwaysOnTop(!!cfg.onTop).catch(() => {});
    if (connection) pendingReconnect = true;
  });
  listen("settings-closed", async () => {
    if (pendingReconnect && running) await connect(); // restart with the new connection settings
    pendingReconnect = false;
  });

  // ------------------------------------------------------------ start
  applyTheme();
  renderFrequency($("freq"), null);
  render({ link: "stopped", source: "", message: "" });
  listen("rig-state", (e) => render(e.payload));
  if (TAURI && cfg.onTop) TAURI.window.getCurrentWindow().setAlwaysOnTop(true).catch(() => {});
  if (cfg.autoConnect) connect();

  // ------------------------------------------------------------ browser mock
  function mockListen(_name, cb) { mockListen.cb = cb; return Promise.resolve(() => {}); }
  async function mockInvoke(cmd) {
    if (cmd === "open_settings") return;
    if (cmd === "list_ports") return [{ name: "/dev/ttyUSB0", description: "FTDI FT232R USB UART" }];
    if (cmd === "stop_monitor") { clearInterval(mockInvoke.t); mockListen.cb?.({ payload: { link: "stopped" } }); return; }
    if (cmd === "start_monitor") {
      clearInterval(mockInvoke.t);
      const t0 = performance.now();
      mockInvoke.t = setInterval(() => {
        const t = ((performance.now() - t0) / 1000) % 20;
        const tx = t > 12;
        mockListen.cb?.({ payload: {
          link: "up", source: "Browser mock", polls_per_sec: 88, errors: 0, message: "",
          freq_hz: 14074000, mode: "DIG USER-U", band: "20m", vfo: "VFO A", tx,
          s_raw: 3 + Math.floor(Math.random() * 6), s_label: "S5", squelched: false, tone_mismatch: false, discr_off: false,
          po: tx ? 11 : 0, swr: tx ? 2 : null, alc: tx ? 1 : null, modulation: tx ? 7 : null, high_swr: false,
          agc: "FAST", nb: true, dsp: false, pbt: false, lock: false, fst: false, vox: false, bk: false, kyr: false,
          split: false, tx_power: "HI", meter_mode: "PWR", antenna: "FRONT", lcd_color: "amber",
        } });
      }, 50);
    }
  }
})();
