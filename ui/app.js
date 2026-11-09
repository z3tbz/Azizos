// Azizos UI — runs in the browser with mock data for preview, and talks to the
// Rust engine through Tauri `invoke` when running inside the built app.
(() => {
  // Tauri 2 exposes the API under __TAURI__.core / .event (withGlobalTauri: true).
  const TAURI = !!(window.__TAURI__ && window.__TAURI__.core);
  const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
  const listen = (ev, cb) => window.__TAURI__.event.listen(ev, cb);

  let lang = (navigator.language || "en").toLowerCase().startsWith("ar") ? "ar" : "en";
  const T = () => window.I18N[lang];
  const CAT = window.CATALOG;
  const selected = new Set(CAT.tweaks.filter(t => t.default).map(t => t.id));
  let reqStatuses = [];

  const STEPS = ["welcome","preflight","tweaks","review","apply","done"];
  let current = "welcome";

  // Text from the system (error output, AV names, paths) is never trusted as HTML.
  const esc = s => String(s == null ? "" : s).replace(/[&<>"']/g,
    c => ({ "&":"&amp;", "<":"&lt;", ">":"&gt;", "\"":"&quot;", "'":"&#39;" }[c]));

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
    renderStepper(); renderPreflight(); renderTweaks(); wireProfiles(); // re-render dynamic text
    if (current === "review") renderReview();
    if (current === "done") renderDone();
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
    const ci = STEPS.indexOf(current);
    STEPS.forEach((s, i) => {
      const done = ci > i;
      const d = document.createElement("div");
      d.className = "step" + (s === current ? " active" : (done ? " done" : ""));
      d.innerHTML = `<span class="num">${done ? "✓" : (i + 1)}</span><span>${T().steps[i]}</span>`;
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
      { requirement:"SupportedBuild",  met:"Pass", detail:"build 26100" },
      { requirement:"DefenderToggled", met:"Fail", detail:"real-time protection is ON" },
      { requirement:"NoThirdPartyAv",  met:"Pass", detail:null },
      { requirement:"PluggedIn",       met:"NotApplicable", detail:"desktop" },
    ];
  }
  const REQ_LABEL = {
    SupportedBuild:  { ar:"إصدار ويندوز مدعوم", en:"Supported Windows build" },
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
        <small>${esc(s.detail)}</small></div>`;
      if (met === "fail") {
        const fix = document.createElement("div");
        fix.className = "fix";
        if (s.requirement === "DefenderToggled") {
          const a = btn("primary", T().fix_auto, () => fixDefender(true));
          const m = btn("ghost", T().fix_manual, () => fixDefender(false));
          fix.append(a, m);
        } else if (s.requirement !== "SupportedBuild") {
          // only the user can resolve these (plug in, uninstall AV) — re-check after
          fix.append(btn("ghost", T().recheck, loadAndRender));
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
      if (TAURI) {
        try { await invoke("run_preflight_fix", { requirement: "DefenderToggled" }); }
        catch (e) { alert(T().fix_unavailable + " (" + e + ")"); await invoke("open_windows_security").catch(() => {}); }
      }
    } else {
      if (TAURI) { await invoke("open_windows_security").catch(() => {}); }
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

  // ---------- profiles (quick presets) ----------
  // balanced = safe defaults (low risk). gaming = everything except high-risk
  // (user still opts into Defender/UAC/etc. deliberately). custom = untouched.
  function applyProfile(name) {
    if (name === "custom") return;
    selected.clear();
    CAT.tweaks.forEach(t => {
      if (name === "balanced" && (t.risk === "low") && t.default) selected.add(t.id);
      if (name === "gaming" && t.risk !== "high") selected.add(t.id);
    });
    document.querySelectorAll(".prof").forEach(b =>
      b.classList.toggle("on", b.dataset.prof === name));
    renderTweaks();
  }
  function wireProfiles() {
    document.querySelectorAll(".prof").forEach(b => b.onclick = () => applyProfile(b.dataset.prof));
  }

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
    const desc = (t.description && t.description[lang]) ? t.description[lang] : `${t.actions_count} ${T().act_count}`;
    meta.innerHTML = `<div class="name">${name}${badges}</div><div class="desc">${desc}</div>`;
    row.append(cb, meta);
    return row;
  }

  // ---------- review ----------
  function renderReview() {
    const sel = CAT.tweaks.filter(t => selected.has(t.id));
    const actions = sel.reduce((n, t) => n + t.actions_count, 0);
    const reboot = sel.filter(t => t.requires_reboot).length;
    const body = document.getElementById("reviewBody");
    body.innerHTML = `<div class="sum">
      <div class="stat"><b>${sel.length}</b><span>${T().sel_count}</span></div>
      <div class="stat"><b>${actions}</b><span>${T().act_count}</span></div>
      <div class="stat"><b>${reboot}</b><span>${T().reboot_needed}</span></div>
    </div>
    <div class="rvlist">${sel.map(t => `<div>• ${(t.name&&t.name[lang])||t.id}</div>`).join("")}</div>`;
  }

  // ---------- apply ----------
  let lastReport = null;
  async function runApply() {
    const bar = document.getElementById("progBar");
    const status = document.getElementById("statusLine");
    const log = document.getElementById("applyLog");
    const pct = document.getElementById("pct");
    const setPct = p => { bar.style.width = p + "%"; pct.innerHTML = p + "<small>%</small>"; };
    log.innerHTML = ""; setPct(0);
    const sel = CAT.tweaks.filter(t => selected.has(t.id));

    if (TAURI) {
      const byId = Object.fromEntries(CAT.tweaks.map(t => [t.id, t]));
      const unlisten = await listen("apply-progress", ({ payload: p }) => {
        const t = byId[p.id];
        const label = (t && t.name && t.name[lang]) || p.id;
        status.textContent = label;
        const row = document.createElement("div");
        row.className = "row " + (p.failed ? "bad" : "ok");
        row.textContent = label + " — " + (p.failed ? T().r_failed : T().r_applied);
        log.appendChild(row); log.scrollTop = log.scrollHeight;
        setPct(Math.round((p.done / Math.max(p.total, 1)) * 100));
      });
      try {
        lastReport = await invoke("run_plan", { selected: sel.map(t => t.id), dryRun: false });
      } catch (e) {
        // the run itself could not start (e.g. unreadable system info): show it on Done
        lastReport = { applied: 0, failed: 1, skipped: 0, failures: [["azizos", 0, String(e)]], archive: null };
      }
      unlisten();
      setPct(100);
      return go("done");
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
      willFail ? failed++ : applied += t.actions_count;
      i++; setPct(Math.round((i / sel.length) * 100));
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
    body.innerHTML = `<div class="mark ${ok ? "ok" : "warn"}" aria-hidden="true"></div>
      <p>${ok ? T().done_ok : T().done_some}</p>
      <div class="sum" style="justify-content:center">
        <div class="stat"><b>${lastReport ? lastReport.applied : 0}</b><span>${T().r_applied}</span></div>
        <div class="stat"><b>${lastReport ? lastReport.failed : 0}</b><span>${T().r_failed}</span></div>
      </div>
      ${fails.length ? `<div class="failbox"><b>${T().failures_title}</b>${
        fails.map(f => `<div class="row">${esc(f[0])} — ${esc(f[2])}</div>`).join("")}</div>` : ""}
      ${(!TAURI || (lastReport && lastReport.archive)) ? `<p class="muted" style="margin-top:16px">${T().archive_note}${
        lastReport && lastReport.archive ? `<br><code class="path">${esc(lastReport.archive)}</code>` : ""}</p>` : ""}`;
  }
  // re-render done whenever we land on it
  const _go = go;
  go = function (s) { _go(s); if (s === "done") renderDone(); };
  document.getElementById("finishBtn").onclick = async () => {
    const auto = document.getElementById("autoRestart").checked;
    if (TAURI) {
      try {
        const r = await invoke("finish", { autoRestart: auto });
        if (!r.restarting) alert(r.optimizer_scheduled ? T().finish_manual : T().finish_manual_no_opt);
      } catch (e) { alert(String(e)); }
    }
    else alert(lang === "ar" ? "في التطبيق الحقيقي: إعادة تشغيل ثم فتح الأوبتمايزر." :
                               "In the real app: restart, then open the Optimizer.");
  };

  // ---------- boot ----------
  loadRequirements().then(() => { applyLang(); go("welcome"); });
})();
