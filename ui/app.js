// Azizos UI — runs in the browser with mock data for preview, and talks to the
// Rust engine through Tauri `invoke` when running inside the built app.
(() => {
  const TAURI = !!(window.__TAURI__ && window.__TAURI__.invoke);
  const invoke = (cmd, args) => window.__TAURI__.invoke(cmd, args);

  let lang = (navigator.language || "en").toLowerCase().startsWith("ar") ? "ar" : "en";
  const T = () => window.I18N[lang];
  const CAT = window.CATALOG;
  const selected = new Set(CAT.tweaks.filter(t => t.default).map(t => t.id));
  let reqStatuses = [];

  const STEPS = ["welcome","preflight","tweaks","review","apply","done"];
  let current = "welcome";

  function btn(cls, label, onclick) {
    const b = document.createElement("button");
    b.className = cls; b.textContent = label; b.onclick = onclick;
    return b;
  }

  // ---------- i18n / theme ----------
  function applyLang() {
    document.documentElement.lang = lang;
    document.documentElement.dir = lang === "ar" ? "rtl" : "ltr";
    document.getElementById("langBtn").textContent = lang === "ar" ? "EN" : "ع";
    document.querySelectorAll("[data-i18n]").forEach(el => {
      const v = T()[el.getAttribute("data-i18n")];
      if (v != null) el.textContent = v;
    });
    renderStepper(); renderPreflight(); renderTweaks(); // re-render dynamic text
  }
  document.getElementById("langBtn").onclick = () => { lang = lang === "ar" ? "en" : "ar"; applyLang(); };
  document.getElementById("themeBtn").onclick = () => {
    const r = document.documentElement;
    r.dataset.theme = r.dataset.theme === "dark" ? "light" : "dark";
  };

  // ---------- navigation ----------
  function renderStepper() {
    const nav = document.getElementById("stepper");
    nav.innerHTML = "";
    STEPS.forEach((s, i) => {
      const d = document.createElement("div");
      d.className = "step" + (s === current ? " active" : (STEPS.indexOf(current) > i ? " done" : ""));
      d.textContent = (i + 1) + ". " + T().steps[i];
      nav.appendChild(d);
    });
  }
  function go(screen) {
    current = screen;
    document.querySelectorAll(".screen").forEach(s => s.hidden = s.dataset.screen !== screen);
    renderStepper();
    if (screen === "review") renderReview();
    if (screen === "apply") runApply();
    if (screen === "done") {} // filled by runApply
    window.scrollTo(0, 0);
  }
  document.querySelectorAll("[data-next]").forEach(b => b.onclick = () => go(b.getAttribute("data-next")));

  // ---------- pre-flight ----------
  async function loadRequirements() {
    if (TAURI) { try { reqStatuses = await invoke("check_requirements"); return; } catch(e){} }
    // mock: defender real-time ON (fail), no third-party AV (pass), desktop (n/a)
    reqStatuses = [
      { requirement:"DefenderToggled", met:"Fail", detail:"real-time protection is ON" },
      { requirement:"NoThirdPartyAv",  met:"Pass", detail:null },
      { requirement:"PluggedIn",       met:"NotApplicable", detail:"desktop" },
    ];
  }
  const REQ_LABEL = {
    DefenderToggled: { ar:"حماية Defender اللحظية", en:"Defender real-time protection" },
    NoThirdPartyAv:  { ar:"لا يوجد مضاد فيروسات آخر", en:"No third-party antivirus" },
    PluggedIn:       { ar:"موصول بالشاحن", en:"On AC power" },
  };
  function renderPreflight() {
    const list = document.getElementById("reqList");
    if (!list) return;
    list.innerHTML = "";
    let anyFail = false;
    reqStatuses.forEach(s => {
      const met = s.met.toLowerCase();
      if (met === "fail") anyFail = true;
      const cls = met === "pass" ? "pass" : (met === "fail" ? "fail" : "na");
      const row = document.createElement("div");
      row.className = "req " + cls;
      row.innerHTML = `<span class="dot"></span>
        <div class="txt"><b>${REQ_LABEL[s.requirement][lang]}</b>
        <small>${s.detail ? s.detail : ""}</small></div>`;
      if (met === "fail") {
        const fix = document.createElement("div");
        fix.className = "fix";
        if (s.requirement === "DefenderToggled") {
          const a = btn("primary", T().fix_auto, () => fixDefender(true));
          const m = btn("ghost", T().fix_manual, () => fixDefender(false));
          fix.append(a, m);
        } else {
          const m = btn("ghost", T().fix_manual, loadAndRender);
          fix.append(m);
        }
        row.appendChild(fix);
      }
      list.appendChild(row);
    });
    const next = document.getElementById("preNext");
    if (next) { next.disabled = anyFail; next.title = anyFail ? T().need_fix : T().all_ok; }
  }
  async function fixDefender(auto) {
    if (auto) {
      if (!confirm(T().consent)) return;
      if (TAURI) { try { await invoke("disable_defender_scripts"); } catch(e){} }
    } else {
      if (TAURI) { try { await invoke("open_windows_security"); } catch(e){} }
    }
    if (TAURI) { await loadAndRender(); }
    else {
      // preview: mark it resolved so the flow is demoable
      const d = reqStatuses.find(s => s.requirement === "DefenderToggled");
      if (d) { d.met = "Pass"; d.detail = null; }
      renderPreflight();
    }
  }
  async function loadAndRender(){ await loadRequirements(); renderPreflight(); }

  // ---------- tweaks ----------
  function renderTweaks() {
    const host = document.getElementById("catList");
    if (!host) return;
    host.innerHTML = "";
    const cats = CAT.categories;
    Object.keys(cats).forEach(catKey => {
      const items = CAT.tweaks.filter(t => t.category === catKey);
      if (!items.length) return;
      const box = document.createElement("div");
      box.className = "cat";
      box.innerHTML = `<h2>${cats[catKey][lang]}</h2>`;
      items.forEach(t => box.appendChild(tweakRow(t)));
      host.appendChild(box);
    });
  }
  function tweakRow(t) {
    const row = document.createElement("label");
    row.className = "tweak";
    const cb = document.createElement("input");
    cb.type = "checkbox"; cb.checked = selected.has(t.id);
    cb.onchange = () => cb.checked ? selected.add(t.id) : selected.delete(t.id);
    const meta = document.createElement("div");
    meta.className = "meta";
    const name = (t.name && t.name[lang]) || t.id;
    let badges = "";
    if (t.risk === "high") badges += `<span class="badge high">${T().risk_high}</span>`;
    if (t.risk === "medium") badges += `<span class="badge medium">●</span>`;
    if (t.requires_reboot) badges += `<span class="badge reboot">${T().reboot}</span>`;
    const desc = (t.description && t.description[lang]) ? t.description[lang] : `${t.actions.length} ${T().act_count}`;
    meta.innerHTML = `<div class="name">${name}${badges}</div><div class="desc">${desc}</div>`;
    row.append(cb, meta);
    return row;
  }

  // ---------- review ----------
  function renderReview() {
    const sel = CAT.tweaks.filter(t => selected.has(t.id));
    const actions = sel.reduce((n, t) => n + t.actions.length, 0);
    const reboot = sel.filter(t => t.requires_reboot).length;
    const body = document.getElementById("reviewBody");
    body.innerHTML = `<div class="sum">
      <div class="stat"><b>${sel.length}</b>${T().sel_count}</div>
      <div class="stat"><b>${actions}</b>${T().act_count}</div>
      <div class="stat"><b>${reboot}</b>${T().reboot_needed}</div>
    </div>
    <div class="rvlist">${sel.map(t => `<div>• ${(t.name&&t.name[lang])||t.id}</div>`).join("")}</div>`;
  }

  // ---------- apply ----------
  let lastReport = null;
  async function runApply() {
    const bar = document.getElementById("progBar");
    const status = document.getElementById("statusLine");
    const log = document.getElementById("applyLog");
    log.innerHTML = ""; bar.style.width = "0%";
    const sel = CAT.tweaks.filter(t => selected.has(t.id));

    if (TAURI) {
      try {
        const ids = sel.map(t => t.id);
        lastReport = await invoke("run_plan", { selected: ids, dryRun: false });
        bar.style.width = "100%"; return go("done");
      } catch (e) { status.textContent = String(e); }
      return;
    }

    // ---- browser simulation ----
    let applied = 0, failed = 0, i = 0;
    for (const t of sel) {
      status.textContent = (t.name && t.name[lang]) || t.id;
      // simulate one deterministic failure to show the report path
      const willFail = t.id === "disable-wu";
      const row = document.createElement("div");
      row.className = "row " + (willFail ? "bad" : "ok");
      row.textContent = ((t.name && t.name[lang]) || t.id) + " — " +
        (willFail ? T().r_failed : T().r_applied);
      log.appendChild(row); log.scrollTop = log.scrollHeight;
      willFail ? failed++ : applied += t.actions.length;
      i++; bar.style.width = Math.round((i / sel.length) * 100) + "%";
      await new Promise(r => setTimeout(r, 180));
    }
    lastReport = {
      applied, failed,
      failures: failed ? [["disable-wu", 0, "simulated error 0x80070005"]] : []
    };
    setTimeout(() => go("done"), 400);
  }

  // ---------- done ----------
  function renderDone() {
    const ok = !lastReport || lastReport.failed === 0;
    const body = document.getElementById("doneBody");
    const fails = (lastReport && lastReport.failures) || [];
    body.innerHTML = `<div class="big">${ok ? "✅" : "⚠️"}</div>
      <p>${ok ? T().done_ok : T().done_some}</p>
      <div class="sum">
        <div class="stat"><b>${lastReport ? lastReport.applied : 0}</b>${T().r_applied}</div>
        <div class="stat"><b>${lastReport ? lastReport.failed : 0}</b>${T().r_failed}</div>
      </div>
      ${fails.length ? `<div class="failbox"><b>${T().failures_title}</b>${
        fails.map(f => `<div class="row">${f[0]} — ${f[2]}</div>`).join("")}</div>` : ""}`;
  }
  // re-render done whenever we land on it
  const _go = go;
  go = function (s) { _go(s); if (s === "done") renderDone(); };
  document.getElementById("finishBtn").onclick = async () => {
    const auto = document.getElementById("autoRestart").checked;
    if (TAURI) { try { await invoke("finish", { autoRestart: auto }); } catch(e){} }
    else alert(lang === "ar" ? "في التطبيق الحقيقي: إعادة تشغيل ثم فتح الأوبتمايزر." :
                               "In the real app: restart, then open the Optimizer.");
  };

  // ---------- boot ----------
  loadRequirements().then(() => { applyLang(); go("welcome"); });
})();
