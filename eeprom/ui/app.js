// 817 EEPROM UI.
(function () {
  const $ = (id) => document.getElementById(id);
  const T = window.__TAURI__;
  const invoke = T.core.invoke;
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

  const CAL_NAMES = ("HF1RXG HF2RXG HF3RXG 50MRXG VHFRXG UHFRXG SSB-S9 SSB-FS FM-S1 FM-FS DISC-L DISC-H FM-TH1 FM-TH2 FM-TI1 FM-TI2 " +
    "VCC HF1-IC HF2-IC HF3-IC 50M-IC VHF-IC UHF-IC HF1-HI HF1-L3 HF1-L2 HF1-L1 HF2-HI HF2-L3 HF2-L2 HF2-L1 HF3-HI HF3-L3 HF3-L2 HF3-L1 " +
    "50M-HI 50M-L3 50M-L2 50M-L1 VHF-HI VHF-L3 VHF-L2 VHF-L1 UHF-HI UHF-L3 UHF-L2 UHF-L1 HF1TXG HF2TXG HF3TXG 50MTXG VHFTXG UHFTXG " +
    "HF1POM HF2POM HF3POM 50MPOM VHFPOM UHFPOM ALC1-M ALC-M HF1-RV HF2-RV HF3-RV 50M-RV VHF-RV UHF-RV CW-CAR AM-CAR DEV-W DEV-N " +
    "M-MTR CTCSS DCS LSB-CP USB-CP").split(" ");

  // ------------------------------------------------------------ settings
  let prefs = { source: "demo", baud: "38400" };
  try { prefs = { ...prefs, ...JSON.parse(localStorage.getItem("ft817ee") || "{}") }; } catch (_) {}
  const savePrefs = () => { try { localStorage.setItem("ft817ee", JSON.stringify(prefs)); } catch (_) {} };

  function source() {
    return prefs.source === "demo" ? { kind: "demo" } : { kind: "serial", port: prefs.source, baud: Number(prefs.baud) };
  }

  async function refreshPorts() {
    const sel = $("source");
    let ports = [];
    try { ports = await invoke("list_ports"); } catch (_) {}
    sel.innerHTML = "";
    for (const p of ports) {
      const o = document.createElement("option");
      o.value = p.name;
      o.textContent = p.description ? `${p.name} — ${p.description}` : p.name;
      sel.appendChild(o);
    }
    if (prefs.source !== "demo" && !ports.some((p) => p.name === prefs.source)) {
      const o = document.createElement("option");
      o.value = prefs.source; o.textContent = `${prefs.source} (not present)`;
      sel.appendChild(o);
    }
    const d = document.createElement("option");
    d.value = "demo"; d.textContent = "Demo (simulated FT-817)";
    sel.appendChild(d);
    sel.value = prefs.source;
    syncDemo();
  }
  function syncDemo() {
    $("demo-change").classList.toggle("hidden", prefs.source !== "demo");
    $("demo-other").classList.toggle("hidden", prefs.source !== "demo");
    $("baud").disabled = prefs.source === "demo";
  }
  $("source").addEventListener("change", (e) => { prefs.source = e.target.value; savePrefs(); syncDemo(); invalidatePlan(); });
  $("baud").addEventListener("change", (e) => { prefs.baud = e.target.value; savePrefs(); });
  $("baud").value = prefs.baud;
  $("refresh").addEventListener("click", refreshPorts);

  // ------------------------------------------------------------ progress / busy
  let busy = false;
  function setBusy(b, label) {
    busy = b;
    $("progress").classList.toggle("hidden", !b);
    if (b) { $("pphase").textContent = label || "Working…"; $("pcount").textContent = ""; $("pfill").style.width = "0"; }
    for (const id of ["backup-btn", "save-as-btn", "open-btn", "compare-btn", "restore-btn", "refresh", "source", "baud", "demo-change", "demo-other", "other-text"]) {
      const el = $(id);
      if (b) { el.dataset.wasDisabled = el.disabled ? "1" : ""; el.disabled = true; }
      else { el.disabled = el.dataset.wasDisabled === "1"; }
    }
    if (!b) syncDemo();
  }
  T.event.listen("progress", (e) => {
    const p = e.payload;
    $("pphase").textContent = p.phase;
    $("pcount").textContent = p.total ? `${Math.round((p.done / p.total) * 100)}%` : "";
    $("pfill").style.width = p.total ? `${(p.done / p.total) * 100}%` : "0";
  });
  $("cancel-btn").addEventListener("click", () => invoke("cancel"));

  function show(id, cls, html) {
    const el = $(id);
    el.className = "result " + (cls || "");
    el.innerHTML = html;
  }
  const errorBox = (id, e) => show(id, "err", `<h3>Stopped</h3><div>${esc(e)}</div>`);

  // ------------------------------------------------------------ backup
  let calText = "";
  function renderCal(info) {
    const g = $("cal-grid");
    g.innerHTML = "";
    const lines = [];
    info.calibration.forEach((v, i) => {
      let shown = v;
      if (i >= 74) shown = v & 0x80 ? -(v & 0x7f) : v; // LSB-CP / USB-CP are signed
      const d = document.createElement("div");
      d.innerHTML = `<b>${String(i + 1).padStart(2, "0")} ${esc(CAL_NAMES[i])}</b><span>${shown}</span>`;
      g.appendChild(d);
      lines.push(`${String(i + 1).padStart(2, "0")} ${CAL_NAMES[i].padEnd(7)} ${shown}`);
    });
    lines.push(`VCC calibration byte (0x53): ${info.vcc_cal}`);
    calText = `FT-817/818 soft calibration, read ${info.created}\n` + lines.join("\n") + "\n";
    $("cal-wrap").classList.remove("hidden");
  }
  $("copy-cal").addEventListener("click", async (e) => {
    e.preventDefault();
    try { await navigator.clipboard.writeText(calText); e.target.textContent = "Copied"; setTimeout(() => (e.target.textContent = "Copy as text"), 1500); } catch (_) {}
  });

  $("backup-btn").addEventListener("click", async () => {
    setBusy(true, "Connecting…");
    try {
      const info = await invoke("backup", { source: source() });
      show("backup-result", "ok", `<h3>Backup complete</h3><dl class="kv">
        <dt>Read at</dt><dd>${esc(info.created)}</dd>
        <dt>Size</dt><dd>${info.length} bytes (0x0000–0x${(info.length - 1).toString(16).toUpperCase().padStart(4, "0")})</dd>
        <dt>Version bytes</dt><dd class="mono">${esc(info.version_bytes)}</dd>
        <dt>SHA-256</dt><dd class="mono">${esc(info.sha256)}</dd>
        <dt>Saved to</dt><dd class="mono">${esc(info.path)}</dd></dl>`);
      renderCal(info);
      $("save-as-btn").dataset.wasDisabled = "";
    } catch (e) { errorBox("backup-result", e); }
    setBusy(false);
  });

  $("save-as-btn").addEventListener("click", async () => {
    const stamp = new Date().toISOString().slice(0, 10);
    const path = await T.dialog.save({ defaultPath: `FT817_EEPROM_${stamp}.817ee`, filters: [{ name: "817 EEPROM backup", extensions: ["817ee"] }] });
    if (!path) return;
    try { await invoke("save_backup_as", { path }); show("backup-result", "ok", $("backup-result").innerHTML + `<p class="note">Copy saved to <code>${esc(path)}</code></p>`); }
    catch (e) { errorBox("backup-result", e); }
  });

  // ------------------------------------------------------------ restore
  let otherRadio = false;
  function invalidatePlan() {
    $("plan-result").classList.add("hidden");
    $("confirm-wrap").classList.add("hidden");
    $("confirm").checked = false;
    $("restore-btn").disabled = true;
    $("other-wrap").classList.add("hidden");
    $("other-text").value = "";
    otherRadio = false;
  }

  function updateRestoreBtn() {
    const typed = $("other-text").value.trim().toUpperCase() === "DIFFERENT RADIO";
    $("restore-btn").disabled = !$("confirm").checked || (otherRadio && !typed);
  }

  function otherRadioWarning(p) {
    const rows = p.cal_differences.map((d) =>
      `<tr><td>${String(d.item).padStart(2, "0")} ${esc(CAL_NAMES[d.item - 1])}</td><td>${d.radio}</td><td>→ ${d.backup}</td></tr>`).join("");
    return `<div class="danger-box"><h3>⚠ This is most likely a backup from another radio</h3>
      <p>${p.cal_differences.length} of the 76 calibration values in this backup differ from the radio connected now. A radio's own backup normally matches its calibration exactly, because the calibration doesn't change in normal use.</p>
      <p>Every FT-817 is calibrated individually at the factory. Restoring another radio's calibration can make the S-meter, power output, meters and deviation wrong on this radio. If you have more than one radio, check that you opened the right file.</p>
      <details><summary>Show the ${p.cal_differences.length} differing calibration values</summary><div class="changes"><table><tr><th>Item</th><th>Radio now</th><th>Backup</th></tr>${rows}</table></div></details></div>`;
  }

  $("open-btn").addEventListener("click", async () => {
    const path = await T.dialog.open({ multiple: false, directory: false, defaultPath: dir || undefined, filters: [{ name: "817 EEPROM backup", extensions: ["817ee"] }] });
    if (!path) return;
    invalidatePlan();
    $("restore-result").classList.add("hidden");
    try {
      const info = await invoke("load_backup", { path });
      show("loaded-result", "ok", `<h3>Backup file checked</h3><dl class="kv">
        <dt>File</dt><dd class="mono">${esc(info.path)}</dd>
        <dt>Taken</dt><dd>${esc(info.created)}${info.note ? ` · ${esc(info.note)}` : ""}</dd>
        <dt>Size</dt><dd>${info.length} bytes · version bytes <span class="mono">${esc(info.version_bytes)}</span></dd>
        <dt>Integrity</dt><dd>SHA-256 matches the contents</dd></dl>`);
      $("compare-btn").disabled = false;
    } catch (e) {
      errorBox("loaded-result", e);
      $("compare-btn").disabled = true;
    }
  });

  $("compare-btn").addEventListener("click", async () => {
    invalidatePlan();
    $("restore-result").classList.add("hidden");
    setBusy(true, "Reading the radio…");
    try {
      const p = await invoke("prepare_restore", { source: source() });
      if (p.writes === 0) {
        show("plan-result", "ok", `<h3>The radio already matches this backup</h3><p class="note">Nothing to restore. A pre-restore backup was still saved to <code>${esc(p.pre_restore_backup)}</code>.</p>`);
      } else {
        const r = p.regions;
        const rows = p.changes.map((w) => `<tr><td>0x${w.addr.toString(16).toUpperCase().padStart(4, "0")}</td><td>${region(w.addr)}</td><td>${hex(w.was)}</td><td>→ ${hex(w.data)}</td></tr>`).join("");
        otherRadio = p.likely_other_radio;
        show("plan-result", "warn", `${otherRadio ? otherRadioWarning(p) : ""}<h3>${p.bytes_changed} byte${p.bytes_changed === 1 ? "" : "s"} differ from the backup</h3>
          <div class="regions">
            <span class="${r.calibration ? "cal" : ""}">Calibration: ${r.calibration}</span>
            <span>Settings: ${r.settings}</span><span>VFOs/home: ${r.vfos}</span><span>Memories: ${r.memories}</span>
          </div>
          ${p.cat_rate_kept ? `<p class="note">The backup has a different CAT baud rate. The radio's current rate will be kept so the connection stays up; you can change it in the radio's menu afterwards.</p>` : ""}
          <p class="note">Pre-restore backup saved to <code>${esc(p.pre_restore_backup)}</code>. To undo this restore, restore that file.</p>
          <details><summary>Show the ${p.writes} write${p.writes === 1 ? "" : "s"}</summary><div class="changes"><table><tr><th>Address</th><th>Area</th><th>Radio now</th><th>Backup</th></tr>${rows}</table></div></details>`);
        $("restore-btn").textContent = `Restore ${p.bytes_changed} byte${p.bytes_changed === 1 ? "" : "s"}`;
        $("other-wrap").classList.toggle("hidden", !otherRadio);
        $("confirm-wrap").classList.remove("hidden");
        updateRestoreBtn();
      }
    } catch (e) { errorBox("plan-result", e); }
    setBusy(false);
  });

  $("confirm").addEventListener("change", updateRestoreBtn);
  $("other-text").addEventListener("input", updateRestoreBtn);

  $("restore-btn").addEventListener("click", async () => {
    setBusy(true, "Writing…");
    try {
      const rep = await invoke("run_restore", { source: source(), otherRadioAck: otherRadio });
      if (rep.final_mismatches.length === 0) {
        show("restore-result", "ok", `<h3>Restore complete and verified</h3><p class="note">Wrote ${rep.written} byte pair${rep.written === 1 ? "" : "s"}${rep.retried ? ` (${rep.retried} retried)` : ""}. A full read-back matches the backup. You can power-cycle the radio now.</p>`);
      } else {
        const list = rep.final_mismatches.slice(0, 20).map((a) => "0x" + a.toString(16).toUpperCase().padStart(4, "0")).join(", ");
        show("restore-result", "warn", `<h3>Restore finished, but ${rep.final_mismatches.length} byte(s) don't match</h3><p class="note">Addresses: <code>${list}</code>. The radio may have changed them itself (for example if a knob was turned). Compare again to see what's left; your pre-restore backup is saved.</p>`);
      }
    } catch (e) { errorBox("restore-result", e); }
    invalidatePlan();
    setBusy(false);
  });

  $("demo-change").addEventListener("click", async () => {
    const n = await invoke("demo_change_radio", { otherRadio: false });
    invalidatePlan();
    $("demo-change").textContent = `Changed ${n} bytes on the simulated radio ✓`;
    setTimeout(() => ($("demo-change").textContent = "Demo: change the simulated radio"), 2500);
  });

  $("demo-other").addEventListener("click", async () => {
    const n = await invoke("demo_change_radio", { otherRadio: true });
    invalidatePlan();
    $("demo-other").textContent = `Changed ${n} calibration values ✓`;
    setTimeout(() => ($("demo-other").textContent = "Demo: make it another radio"), 2500);
  });

  function hex(pair) { return pair.map((b) => b.toString(16).toUpperCase().padStart(2, "0")).join(" "); }
  function region(a) {
    if (a <= 0x06) return "header";
    if (a <= 0x53) return "calibration";
    if (a <= 0x7c) return "settings";
    if (a <= 0x44e) return "VFOs/home";
    return "memories";
  }

  // ------------------------------------------------------------ start
  let dir = "";
  invoke("get_backups_dir").then((d) => { dir = d; $("dir").textContent = d; }).catch(() => {});
  refreshPorts();
})();
