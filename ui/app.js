// Azizos UI. Inside the built app it talks to the Rust engine through Tauri
// (`window.__TAURI__`); opened in a plain browser it runs on mock data so the
// design can be previewed (scripts/serve-ui.ps1).
(() => {
  "use strict";
  const TAURI = !!(window.__TAURI__ && window.__TAURI__.core);
  const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
  const listen = (ev, cb) => window.__TAURI__.event.listen(ev, cb);
  const $ = id => document.getElementById(id);

  let lang = (navigator.language || "en").toLowerCase().startsWith("ar") ? "ar" : "en";
  const T = () => window.I18N[lang];
  const CAT = window.CATALOG;
  const selected = new Set(CAT.tweaks.filter(t => t.default).map(t => t.id));

  // Text that comes from the system (errors, AV names, paths) is never HTML.
  const esc = s => String(s == null ? "" : s).replace(/[&<>"']/g,
    c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", "\"": "&quot;", "'": "&#39;" }[c]));
  const nameOf = t => (t && t.name && t.name[lang]) || (t && t.id) || "";
  const btn = (cls, label, onclick) => {
    const b = document.createElement("button");
    b.type = "button"; b.className = cls; b.textContent = label; b.onclick = onclick;
    return b;
  };

  // ---------- pages (category order = AME-style paging) ----------
  const ORDER = ["base", "performance", "services", "debloat", "privacy", "appearance", "devices", "updates", "security"];
  const catOf = t => t.id.startsWith("core-") ? "base" : t.category;
  const PAGES = ORDER.filter(c => CAT.tweaks.some(t => catOf(t) === c));
  const catLabel = c => c === "base" ? T().cat_base : ((CAT.categories[c] || {})[lang] || c);
  let page = 0;

  const STEPS = ["welcome", "preflight", "tweaks", "review", "apply", "done"];
  let current = "welcome";
  let reqStatuses = [];
  let sys = null;
  let lastReport = null;

  // ---------- language / theme ----------
  function applyLang() {
    const html = document.documentElement;
    html.lang = lang; html.dir = lang === "ar" ? "rtl" : "ltr";
    $("langBtn").textContent = lang === "ar" ? "EN" : "ع";
    document.querySelectorAll("[data-i18n]").forEach(el => {
      const v = T()[el.getAttribute("data-i18n")];
      if (typeof v === "string") el.textContent = v;
    });
    renderRail(); renderSys(); renderPreflight(); renderOptions(); renderSel();
    if (current === "review") renderReview();
    if (current === "done") renderDone();
  }
  $("langBtn").onclick = () => { lang = lang === "ar" ? "en" : "ar"; applyLang(); };
  $("themeBtn").onclick = () => {
    const r = document.documentElement;
    r.dataset.theme = r.dataset.theme === "dark" ? "light" : "dark";
  };

  // ---------- step rail ----------
  function renderRail() {
    const ol = $("rail"); ol.innerHTML = "";
    const ci = STEPS.indexOf(current);
    STEPS.forEach((s, i) => {
      const li = document.createElement("li");
      li.className = i < ci ? "done" : (i === ci ? "now" : "");
      li.innerHTML = `<i>${i < ci ? "✓" : String(i + 1).padStart(2, "0")}</i><span>${T().steps[i]}</span>`;
      ol.appendChild(li);
    });
  }
  function go(screen) {
    current = screen;
    document.querySelectorAll(".screen").forEach(s => s.hidden = s.dataset.screen !== screen);
    $("pbSel").hidden = !(screen === "tweaks" || screen === "review");
    renderRail();
    if (screen === "tweaks") renderOptions();
    if (screen === "review") renderReview();
    if (screen === "apply") runApply();
    if (screen === "done") renderDone();
  }
  document.querySelectorAll("[data-next]").forEach(b => b.onclick = () => go(b.getAttribute("data-next")));

  // ---------- playbook panel: this PC ----------
  async function loadSys() {
    if (TAURI) { try { sys = await invoke("detect_system"); } catch (e) { sys = null; } }
    else sys = { build: 26100, edition: "pro", device: "desktop", gpu: "nvidia", disk: "ssd" };
  }
  function renderSys() {
    $("pbTweaks").textContent = CAT.tweaks.length;
    const box = $("sysInfo");
    if (!sys) { box.innerHTML = `<div class="kv"><span>—</span></div>`; return; }
    const up = s => (s && s !== "any") ? String(s).toUpperCase() : T().any;
    const dev = sys.device === "laptop" ? T().dev_laptop : (sys.device === "desktop" ? T().dev_desktop : T().any);
    box.innerHTML = [
      [T().sys_build, `${esc(sys.build)} · ${esc(String(sys.edition).toUpperCase())}`],
      [T().sys_gpu, esc(up(sys.gpu))],
      [T().sys_disk, esc(up(sys.disk))],
      [T().sys_device, esc(dev)],
    ].map(([k, v]) => `<div class="kv"><span>${k}</span><b>${v}</b></div>`).join("");
  }
  function renderSel() {
    $("selCount").textContent = selected.size;
    $("selTotal").textContent = CAT.tweaks.length;
    $("selMeter").style.width = (selected.size / Math.max(CAT.tweaks.length, 1) * 100) + "%";
  }

  // ---------- requirements ----------
  const REQ = {
    SupportedBuild:  { icon: "WIN", ar: "إصدار ويندوز مدعوم", en: "Supported Windows build" },
    DefenderToggled: { icon: "DEF", ar: "حماية Defender اللحظية", en: "Defender real-time protection" },
    NoThirdPartyAv:  { icon: "AV",  ar: "لا يوجد مضاد فيروسات آخر", en: "No third-party antivirus" },
    PluggedIn:       { icon: "AC",  ar: "موصول بالشاحن", en: "Plugged into power" },
  };
  async function loadRequirements() {
    if (TAURI) { try { reqStatuses = await invoke("check_requirements"); return; } catch (e) { /* keep last */ } }
    else reqStatuses = [
      { requirement: "SupportedBuild", met: "Pass", detail: "build 26100" },
      { requirement: "DefenderToggled", met: "Fail", detail: "real-time protection is ON" },
      { requirement: "NoThirdPartyAv", met: "Pass", detail: null },
      { requirement: "PluggedIn", met: "NotApplicable", detail: "desktop" },
    ];
  }
  function renderPreflight() {
    const grid = $("reqList"); grid.innerHTML = "";
    let anyFail = false;
    reqStatuses.forEach(s => {
      const met = String(s.met).toLowerCase();
      const cls = met === "pass" ? "pass" : (met === "fail" ? "fail" : "na");
      if (cls === "fail") anyFail = true;
      const meta = REQ[s.requirement] || { icon: "?", ar: s.requirement, en: s.requirement };
      const card = document.createElement("div");
      card.className = "req " + cls;
      card.innerHTML = `<div class="req-ic">${meta.icon}</div>
        <div class="req-txt"><b>${meta[lang]}</b><small>${esc(s.detail || "")}</small></div>
        <span class="tag ${cls}">${cls === "pass" ? T().req_pass : cls === "fail" ? T().req_fail : T().req_na}</span>`;
      if (cls === "fail") {
        const act = document.createElement("div");
        act.className = "req-act";
        if (s.requirement === "DefenderToggled") {
          act.append(btn("primary sm", T().fix_auto, () => fixDefender(true)),
                     btn("ghost sm", T().fix_manual, () => fixDefender(false)));
        } else if (s.requirement !== "SupportedBuild") {
          act.append(btn("ghost sm", T().recheck, loadAndRender));
        }
        card.appendChild(act);
      }
      grid.appendChild(card);
    });
    $("preNext").disabled = anyFail;
    $("preHint").textContent = reqStatuses.length ? (anyFail ? T().need_fix : T().all_ok) : "";
    $("preHint").className = "navhint " + (anyFail ? "bad" : "ok");
  }
  async function fixDefender(auto) {
    if (auto) {
      if (!confirm(T().consent)) return;
      if (TAURI) {
        try { await invoke("run_preflight_fix", { requirement: "DefenderToggled" }); }
        catch (e) { alert(T().fix_unavailable + " (" + e + ")"); await invoke("open_windows_security").catch(() => {}); }
      }
    } else if (TAURI) {
      await invoke("open_windows_security").catch(() => {});
    }
    if (TAURI) return loadAndRender();
    const d = reqStatuses.find(s => s.requirement === "DefenderToggled"); // preview only
    if (d) { d.met = "Pass"; d.detail = null; }
    renderPreflight();
  }
  async function loadAndRender() { await loadRequirements(); renderPreflight(); }

  // ---------- options ----------
  function applyProfile(name) {
    if (name !== "custom") {
      selected.clear();
      CAT.tweaks.forEach(t => {
        if (name === "balanced" && t.risk === "low" && t.default) selected.add(t.id);
        if (name === "gaming" && t.risk !== "high") selected.add(t.id);
      });
    }
    document.querySelectorAll(".prof").forEach(b => b.classList.toggle("on", b.dataset.prof === name));
    renderOptions(); renderSel();
  }
  document.querySelectorAll(".prof").forEach(b => b.onclick = () => applyProfile(b.dataset.prof));

  function renderOptions() {
    const cat = PAGES[page];
    $("catTitle").textContent = catLabel(cat);
    $("pageNo").textContent = `${T().page} ${page + 1} ${T().of} ${PAGES.length}`;

    const tabs = $("catTabs"); tabs.innerHTML = "";
    PAGES.forEach((c, i) => {
      const items = CAT.tweaks.filter(t => catOf(t) === c);
      const on = items.filter(t => selected.has(t.id)).length;
      const b = btn("tab" + (i === page ? " now" : ""), "", () => { page = i; renderOptions(); });
      b.innerHTML = `${esc(catLabel(c))}<em>${on}/${items.length}</em>`;
      tabs.appendChild(b);
    });

    const grid = $("cardGrid"); grid.innerHTML = "";
    if (cat === "base") {
      const n = document.createElement("p");
      n.className = "note"; n.textContent = T().base_note;
      grid.appendChild(n);
    }
    CAT.tweaks.filter(t => catOf(t) === cat).forEach(t => grid.appendChild(card(t)));

    $("optHint").textContent = "";
    $("optNext").textContent = page < PAGES.length - 1 ? T().next : T().rv_title;
  }
  function card(t) {
    const el = document.createElement("label");
    const on = selected.has(t.id);
    el.className = "opt" + (on ? " on" : "") + (t.risk === "high" ? " high" : "");
    const kinds = Object.entries(t.kinds || {}).map(([k, n]) =>
      `<span>${esc((T().kinds[k]) || k)} <b>${n}</b></span>`).join("");
    let tags = "";
    if (t.risk === "high") tags += `<span class="tag fail">${T().risk_high}</span>`;
    else if (t.risk === "medium") tags += `<span class="tag warn">${T().risk_medium}</span>`;
    if (t.requires_reboot) tags += `<span class="tag">${T().reboot}</span>`;
    el.innerHTML = `
      <input type="checkbox" ${on ? "checked" : ""} />
      <span class="opt-top"><b>${esc(nameOf(t))}</b><span class="track"><i></i></span></span>
      <span class="opt-desc">${esc((t.description && t.description[lang]) || "")}</span>
      <span class="opt-foot"><span class="kinds">${kinds}</span><span class="tags">${tags}</span></span>`;
    el.querySelector("input").onchange = e => {
      e.target.checked ? selected.add(t.id) : selected.delete(t.id);
      el.classList.toggle("on", e.target.checked);
      document.querySelectorAll(".prof").forEach(b => b.classList.toggle("on", b.dataset.prof === "custom"));
      renderSel();
      const tab = $("catTabs").children[page];
      const items = CAT.tweaks.filter(x => catOf(x) === PAGES[page]);
      if (tab) tab.querySelector("em").textContent = `${items.filter(x => selected.has(x.id)).length}/${items.length}`;
    };
    return el;
  }
  $("optNext").onclick = () => { if (page < PAGES.length - 1) { page++; renderOptions(); } else go("review"); };
  $("optBack").onclick = () => { if (page > 0) { page--; renderOptions(); } else go("preflight"); };

  // ---------- review ----------
  function renderReview() {
    const sel = CAT.tweaks.filter(t => selected.has(t.id));
    const actions = sel.reduce((n, t) => n + t.actions_count, 0);
    const reboot = sel.filter(t => t.requires_reboot).length;
    const high = sel.filter(t => t.risk === "high");
    const groups = PAGES.map(c => [c, sel.filter(t => catOf(t) === c)]).filter(([, l]) => l.length);
    $("reviewBody").innerHTML = `
      <div class="stats">
        <div class="stat"><b>${sel.length}</b><span>${T().sel_count}</span></div>
        <div class="stat"><b>${actions}</b><span>${T().act_count}</span></div>
        <div class="stat"><b>${reboot}</b><span>${T().reboot_needed}</span></div>
      </div>
      ${high.length ? `<div class="warnbox"><b>${T().high_warn}</b> ${high.map(t => esc(nameOf(t))).join(" · ")}</div>` : ""}
      <div class="rv">${groups.map(([c, l]) => `<div class="rv-g"><span class="eyebrow">${esc(catLabel(c))}</span>
        ${l.map(t => `<div class="rv-i"><span>${esc(nameOf(t))}</span><em>${t.actions_count}</em></div>`).join("")}</div>`).join("")}</div>`;
  }

  // ---------- apply ----------
  function logRow(label, state) {
    const log = $("applyLog");
    const row = document.createElement("div");
    row.className = "row " + state;
    const word = { applied: T().r_applied, already: T().r_already, na: T().r_na, failed: T().r_failed }[state] || "";
    row.innerHTML = `<span>${esc(label)}</span><em>${word}</em>`;
    log.appendChild(row); log.scrollTop = log.scrollHeight;
  }
  function setPct(p) { $("dial").style.setProperty("--p", p); $("pct").textContent = p; }

  async function runApply() {
    $("applyLog").innerHTML = ""; setPct(0);
    const sel = CAT.tweaks.filter(t => selected.has(t.id));
    const byId = Object.fromEntries(CAT.tweaks.map(t => [t.id, t]));

    if (TAURI) {
      const unlisten = await listen("apply-progress", ({ payload: p }) => {
        const label = nameOf(byId[p.id]) || p.id;
        $("statusLine").textContent = label;
        logRow(label, p.state || (p.failed ? "failed" : "applied"));
        setPct(Math.round(p.done / Math.max(p.total, 1) * 100));
      });
      try {
        lastReport = await invoke("run_plan", { selected: sel.map(t => t.id), dryRun: false });
      } catch (e) {
        // the run could not start at all (e.g. system info unreadable): report it
        lastReport = { counts: { applied: 0, already: 0, not_applicable: 0, failed: 1, skipped: 0 },
                       failures: [["azizos", 0, String(e)]], archive: null };
      }
      unlisten(); setPct(100);
      return go("done");
    }

    // ---- browser preview: deterministic mix of outcomes ----
    const c = { applied: 0, already: 0, not_applicable: 0, failed: 0, skipped: 0 };
    const failures = [];
    for (let i = 0; i < sel.length; i++) {
      const t = sel[i];
      $("statusLine").textContent = nameOf(t);
      const state = t.id === "disable-wu" ? "failed" : (t.id === "remove-onedrive" ? "already" : (t.id === "disable-print" ? "na" : "applied"));
      if (state === "failed") { c.failed++; c.applied += t.actions_count - 1; failures.push([t.id, 3, "sc config wuauserv: exit code 5 (0x00000005) Access is denied."]); }
      else if (state === "already") c.already += t.actions_count;
      else if (state === "na") c.not_applicable += t.actions_count;
      else { c.applied += Math.ceil(t.actions_count * 0.8); c.already += Math.floor(t.actions_count * 0.15); c.not_applicable += Math.floor(t.actions_count * 0.05); }
      logRow(nameOf(t), state);
      setPct(Math.round((i + 1) / sel.length * 100));
      await new Promise(r => setTimeout(r, 140));
    }
    lastReport = { counts: c, failures, archive: "C:\\Users\\you\\Desktop\\Azizos-Report-20261110-2130" };
    setTimeout(() => go("done"), 350);
  }

  // ---------- done ----------
  function renderDone() {
    const c = (lastReport && lastReport.counts) || { applied: 0, already: 0, not_applicable: 0, failed: 0 };
    const ok = c.failed === 0;
    const fails = (lastReport && lastReport.failures) || [];
    const byId = Object.fromEntries(CAT.tweaks.map(t => [t.id, t]));
    $("doneBody").innerHTML = `
      <div class="donehead"><div class="mark ${ok ? "ok" : "warn"}" aria-hidden="true"></div>
        <div><h1>${ok ? T().done_ok : T().done_some}</h1><p class="lead">${T().done_sub}</p></div></div>
      <div class="stats four">
        <div class="stat ok"><b>${c.applied}</b><span>${T().r_applied}</span></div>
        <div class="stat"><b>${c.already}</b><span>${T().r_already}</span></div>
        <div class="stat"><b>${c.not_applicable}</b><span>${T().r_na}</span></div>
        <div class="stat ${c.failed ? "bad" : ""}"><b>${c.failed}</b><span>${T().r_failed}</span></div>
      </div>
      ${fails.length ? `<div class="failbox"><b>${T().failures_title}</b>${fails.map(f =>
        `<div class="row"><span>${esc(nameOf(byId[f[0]]) || f[0])} · #${f[1] + 1}</span><code>${esc(f[2])}</code></div>`).join("")}</div>` : ""}
      ${lastReport && lastReport.archive ? `<p class="muted small">${T().archive_note}</p><code class="path">${esc(lastReport.archive)}</code>` : ""}`;
  }
  $("finishBtn").onclick = async () => {
    const auto = $("autoRestart").checked;
    if (!TAURI) return alert(lang === "ar" ? "في التطبيق الحقيقي: إعادة تشغيل ثم فتح الأوبتمايزر." : "In the real app: restart, then open the Optimizer.");
    try {
      const r = await invoke("finish", { autoRestart: auto });
      if (!r.restarting) alert(r.optimizer_scheduled ? T().finish_manual : T().finish_manual_no_opt);
    } catch (e) { alert(String(e)); }
  };

  // ---------- boot ----------
  Promise.all([loadRequirements(), loadSys()]).then(() => {
    document.querySelectorAll(".prof").forEach(b => b.classList.toggle("on", b.dataset.prof === "custom"));
    applyLang(); go("welcome");
  });
})();
