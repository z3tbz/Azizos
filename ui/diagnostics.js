// Local diagnostics only. Preserve backend codes; never replace them with a
// fabricated Windows error or upload logs. Loaded before the main UI scripts.
(() => {
  "use strict";
  const native = !!window.__TAURI__?.core;
  const escape = value => String(value ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
  const arabic = () => document.documentElement.lang !== "en";
  let recorded = 0;
  const raw = (name, args) => window.__TAURI__.core.invoke(name, args);
  const unwrap = value => value?.diagnostic || value;
  function format(value) {
    const d = unwrap(value);
    if (!d || typeof d !== "object" || !d.code) return value?.message || String(value);
    return [d.code + " · " + d.category, d.native_code ? `Windows/exit: ${d.native_code}` : "", d.message,
      arabic() ? d.solution_ar : d.solution_en, (value.log_path || d.log_path) ? `Log: ${value.log_path || d.log_path}` : "",
      (value.logging_error || d.logging_error) ? `Logging error: ${value.logging_error || d.logging_error}` : ""].filter(Boolean).join("\n");
  }
  function html(value) {
    const d = unwrap(value);
    if (!d?.code) return `<code>${escape(format(value))}</code>`;
    return `<b class="diagnostic-code">${escape(d.code)} · ${escape(d.category)}</b>${d.native_code ? `<code>Windows/exit: ${escape(d.native_code)}</code>` : ""}<p>${escape(arabic() ? d.solution_ar : d.solution_en)}</p><code>${escape(d.message)}</code>`;
  }
  function show(value) {
    const panel = document.getElementById("diagnosticPanel");
    if (!panel) return;
    panel.hidden = false;
    document.getElementById("diagnosticText").textContent = format(value);
    document.getElementById("diagnosticSummary").textContent = arabic() ? "تعذّر إكمال خطوة — تفاصيل التشخيص" : "A step could not complete — diagnostics";
  }
  async function record(stage, value) {
    show(value);
    if (!native || recorded++ >= 40) return;
    try {
      const error = await raw("log_frontend_error", {stage, message:String(value?.stack || value?.message || value).slice(0,32768)});
      show(error);
    } catch (_) { /* Keep the visible original error if the bridge is unavailable. */ }
  }
  window.AZ_DIAG = {format, html, show, record};
  window.addEventListener("error", event => {
    const message = event.error || event.message || `Resource failed: ${event.target?.src || event.target?.href || "unknown"}`;
    void record("javascript", message);
  }, true);
  window.addEventListener("unhandledrejection", event => { void record("promise", event.reason); });
  const logs = document.getElementById("logsBtn");
  if (logs) {
    logs.hidden = !native;
    logs.onclick = async () => {
      try { await raw("open_diagnostic_logs"); } catch (e) { show(e); }
    };
  }
  const localRepairs = [];
  function repairPreferences() {
    const changed = [];
    try {
      for (const [key, allowed, fallback] of [
        ["azizos-language", ["ar","en"], "ar"], ["azizos-theme", ["light","dark"], "light"]
      ]) {
        const value = localStorage.getItem(key);
        if (value != null && !allowed.includes(value)) { localStorage.setItem(key, fallback); changed.push(key); }
      }
    } catch (error) { void record("preferences_storage", error); }
    if (changed.length) {
      const detail = `Invalid application preferences reset: ${changed.join(", ")}`;
      localRepairs.push({code:"AZ-2003",area:"ui_preferences",status:"repaired",detail});
      if (native) return raw("log_frontend_repair", {detail});
    }
    return Promise.resolve();
  }
  function showRepairs(records) {
    const latest = new Map();
    for (const item of [...(records || []),...localRepairs]) latest.set(item.area,item);
    const panel = document.getElementById("repairPanel");
    if (!panel || !latest.size) return;
    panel.hidden = false;
    document.getElementById("repairSummary").textContent = arabic() ? "نتيجة فحص وإصلاح التطبيق" : "Application checks and repairs";
    const statuses = arabic() ? {repaired:"أُصلح",healthy:"سليم",failed:"تعذّر الإصلاح"} : {repaired:"Repaired",healthy:"Healthy",failed:"Could not repair"};
    const areas = arabic() ? {logging:"سجل التشخيص",embedded_assets:"ملفات التطبيق",ui_preferences:"تفضيلات الواجهة",report_archive:"حفظ التقرير"} : {logging:"Diagnostic log",embedded_assets:"Application files",ui_preferences:"UI preferences",report_archive:"Report storage"};
    document.getElementById("repairResults").innerHTML = [...latest.values()].map(item => `<div class="failure-item"><b>${escape(item.code)} · ${escape(areas[item.area] || item.area)} · ${escape(statuses[item.status] || item.status)}</b><code>${escape(item.detail)}</code></div>`).join("");
  }
  window.AZ_DIAG.showRepairs = showRepairs;
  const repair = document.getElementById("repairBtn");
  if (repair) {
    repair.hidden = !native;
    repair.onclick = async () => {
      repair.disabled = true;
      try {
        await repairPreferences();
        const info = await raw("repair_app");
        showRepairs(info.recoveries);
        if (info.error) show({message:info.error});
        window.dispatchEvent(new Event("azizos-app-repaired"));
      } catch(error) { show(error); }
      finally { repair.disabled = false; }
    };
  }
  repairPreferences().then(async () => {
    if (native) {
      const info = await raw("diagnostic_info");
      if (info.error) show({message:`${arabic() ? "تعذّر حفظ سجل التشخيص" : "Diagnostic logging unavailable"}: ${info.error}`});
      showRepairs(info.recoveries);
    } else { showRepairs([]); }
  }).catch(error => { show(error); });
})();
