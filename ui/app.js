// UI only: Tauri commands and engine action definitions remain unchanged.
(() => {
  "use strict";
  const TAURI = !!(window.__TAURI__ && window.__TAURI__.core);
  const invoke = async (command, args) => {
    try { return await window.__TAURI__.core.invoke(command, args); }
    catch (value) {
      window.AZ_DIAG.show(value);
      const error = new Error(window.AZ_DIAG.format(value));
      error.diagnostic = value?.code ? value : null;
      throw error;
    }
  };
  const $ = id => document.getElementById(id);
  const CAT = window.CATALOG;
  const STEPS = ["welcome", "preflight", "tweaks", "review", "apply", "done"];
  const ORDER = ["base", "performance", "services", "debloat", "privacy", "appearance", "devices", "updates", "security"];
  const categoryOf = t => t.id.startsWith("core-") ? "base" : t.category;
  const categories = ORDER.filter(c => CAT.tweaks.some(t => categoryOf(t) === c));
  const selected = new Set(CAT.tweaks.filter(t => t.default).map(t => t.id));
  const stored = key => { try { return localStorage.getItem(key); } catch (_) { return null; } };
  const save = (key, value) => { try { localStorage.setItem(key, value); } catch (_) {} };
  let lang = stored("azizos-language") === "en" ? "en" : "ar";
  let theme = stored("azizos-theme") === "dark" ? "dark" : "light";
  let current = "welcome", reqStatuses = [], requirementsError = null, sys = null, lastReport = null, running = false;
  const T = () => window.I18N[lang];
  const esc = value => String(value == null ? "" : value).replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
  const nameOf = t => t?.name?.[lang] || t?.id || "";
  const categoryName = c => c === "base" ? T().cat_base : CAT.categories[c]?.[lang] || c;
  const selectedTweaks = () => CAT.tweaks.filter(t => selected.has(t.id));
  document.body.classList.toggle("native-app", TAURI);
  $("demoBadge").hidden = TAURI;
  $("nativeControls").hidden = !TAURI;
  if (TAURI) {
    const control = action => invoke("window_control", {action}).catch(e => window.AZ_DIAG.show(e));
    $("windowMinimize").onclick = () => control("minimize");
    $("windowMaximize").onclick = () => control("maximize");
    $("windowClose").onclick = () => control("close");
    $("stepCaption").onmousedown = event => { if (event.button === 0 && event.detail === 1) control("drag"); };
    $("stepCaption").ondblclick = () => control("maximize");
  }

  function applyTheme() {
    document.documentElement.dataset.theme = theme;
    if (TAURI) invoke("set_window_theme", {theme}).catch(e => window.AZ_DIAG.show(e));
    $("themeBtn").textContent = theme === "light" ? T().theme_dark : T().theme_light;
    $("themeBtn").setAttribute("aria-pressed", String(theme === "dark"));
  }
  function renderRail() {
    const index = STEPS.indexOf(current);
    $("rail").innerHTML = STEPS.map((step, i) => `<li class="${i < index ? "done" : i === index ? "now" : ""}" ${i === index ? 'aria-current="step"' : ""}><i aria-hidden="true">${i < index ? "✓" : i + 1}</i><span>${esc(T().steps[i])}</span></li>`).join("");
    $("stepCaption").textContent = `${T().step} ${index + 1} ${T().of} ${STEPS.length}`;
  }
  function applyLanguage() {
    document.documentElement.lang = lang;
    document.documentElement.dir = lang === "ar" ? "rtl" : "ltr";
    $("langBtn").textContent = lang === "ar" ? "EN" : "عربي";
    $("langBtn").setAttribute("aria-label", lang === "ar" ? "English" : "العربية");
    document.querySelectorAll("[data-i18n]").forEach(el => { const value = T()[el.dataset.i18n]; if (typeof value === "string") el.textContent = value; });
    document.querySelectorAll("[data-i18n-title]").forEach(el => { el.title = T()[el.dataset.i18nTitle]; el.setAttribute("aria-label",el.title); });
    applyTheme(); renderRail(); renderSystem(); renderRequirements(); renderCategorySelect(); renderOptions();
    if (current === "review") renderReview();
    if (current === "done") renderDone();
  }
  $("langBtn").onclick = () => { lang = lang === "ar" ? "en" : "ar"; save("azizos-language", lang); applyLanguage(); };
  $("themeBtn").onclick = () => { theme = theme === "light" ? "dark" : "light"; save("azizos-theme", theme); applyTheme(); };
  function go(screen) {
    if (running || !STEPS.includes(screen)) return;
    if (screen === "tweaks" && (requirementsError || reqStatuses.length !== 4 || reqStatuses.some(s => s.met === "Fail"))) return;
    if ((screen === "review" || screen === "apply") && !selected.size) return;
    current = screen;
    document.body.dataset.stage = screen;
    document.querySelectorAll(".screen").forEach(el => el.hidden = el.dataset.screen !== screen);
    renderRail();
    if (screen === "tweaks") renderOptions();
    if (screen === "review") renderReview();
    if (screen === "done") renderDone();
    $("app").focus({preventScroll:true});
    if (screen === "apply") runApply();
  }
  document.querySelectorAll("[data-next]").forEach(button => button.onclick = () => go(button.dataset.next));

  async function loadSystem() {
    if (TAURI) { try { sys = await invoke("detect_system"); } catch (e) { sys = null; window.AZ_DIAG.show(e); } }
    else sys = {build:26100,edition:"pro",device:"desktop",gpu:"nvidia",disk:"ssd"};
  }
  function renderSystem() {
    const rows = sys ? [
      [T().sys_build, `${sys.build} · ${String(sys.edition).toUpperCase()}`],
      [T().sys_gpu, sys.gpu === "any" ? T().any : String(sys.gpu).toUpperCase()],
      [T().sys_disk, sys.disk === "any" ? T().any : String(sys.disk).toUpperCase()],
      [T().sys_device, sys.device === "laptop" ? T().dev_laptop : sys.device === "desktop" ? T().dev_desktop : T().any]
    ] : [[T().system_details,T().any]];
    $("sysInfo").innerHTML = rows.map(([key,value]) => `<div class="kv"><span>${esc(key)}</span><b>${esc(value)}</b></div>`).join("");
  }
  async function loadRequirements() {
    requirementsError = null;
    if (TAURI) {
      try { reqStatuses = await invoke("check_requirements"); }
      catch (error) { reqStatuses = []; requirementsError = String(error); }
    } else reqStatuses = [
      {requirement:"SupportedBuild",met:"Pass",detail:"build 26100"},
      {requirement:"DefenderToggled",met:"Fail",detail:"real-time protection is ON"},
      {requirement:"NoThirdPartyAv",met:"Pass",detail:null},
      {requirement:"PluggedIn",met:"NotApplicable",detail:"desktop"}
    ];
  }
  function renderRequirements() {
    const names = {SupportedBuild:T().req_windows,DefenderToggled:T().req_defender,NoThirdPartyAv:T().req_av,PluggedIn:T().req_power};
    const notes = {SupportedBuild:T().req_windows_note,DefenderToggled:T().req_defender_note,NoThirdPartyAv:T().req_av_note,PluggedIn:T().req_power_note};
    const grid = $("reqList"); grid.innerHTML = "";
    if (requirementsError) grid.innerHTML = `<div class="req fail"><span class="req-status">!</span><div class="req-text"><b>${esc(T().load_error)}</b><details class="details"><summary>${esc(T().technical)}</summary><code class="technical-error">${esc(requirementsError)}</code></details></div></div>`;
    reqStatuses.forEach(status => {
      const state = status.met === "Pass" ? "pass" : status.met === "NotApplicable" ? "na" : "fail";
      const row = document.createElement("div"); row.className = `req ${state}`;
      row.innerHTML = `<span class="req-status" aria-hidden="true">${state === "pass" ? "✓" : state === "fail" ? "!" : "–"}</span><div class="req-text"><b>${esc(names[status.requirement] || status.requirement)}</b>${state === "fail" ? `<p>${esc(notes[status.requirement] || T().load_error)}</p>` : (state === "na" && status.requirement === "NoThirdPartyAv") ? `<p>${esc(T().req_av_unknown)}</p>` : ""}</div><span class="tag">${state === "pass" ? T().req_pass : state === "fail" ? T().req_fail : T().req_na}</span>`;
      if (state === "fail" && status.requirement === "DefenderToggled") {
        const button = document.createElement("button"); button.type = "button"; button.className = "req-action"; button.textContent = T().req_resolve;
        button.onclick = async () => {
          if (TAURI) { try { await invoke("open_windows_security"); } catch (e) { window.AZ_DIAG.show(e); } }
          else { status.met = "Pass"; status.detail = null; renderRequirements(); }
        };
        row.appendChild(button);
      }
      if (state === "fail" && status.detail) {
        const details = document.createElement("details"); details.className = "details";
        details.innerHTML = `<summary>${esc(T().technical)}</summary><code class="technical-error">${esc(status.detail)}</code>${status.diagnostic ? window.AZ_DIAG.html(status.diagnostic) : ""}`;
        row.querySelector(".req-text").appendChild(details);
      }
      grid.appendChild(row);
    });
    const ready = !requirementsError && reqStatuses.length === 4 && reqStatuses.every(s => s.met === "Pass" || s.met === "NotApplicable");
    $("preNext").disabled = !ready;
    $("preHint").textContent = ready ? T().all_ok : T().need_fix;
    $("preHint").className = `navhint ${ready ? "" : "bad"}`;
  }
  $("recheckBtn").onclick = async () => {
    $("recheckBtn").disabled = true; $("preNext").disabled = true;
    $("recheckBtn").textContent = T().checking;
    try { await loadRequirements(); renderRequirements(); }
    finally { $("recheckBtn").disabled = false; $("recheckBtn").textContent = T().recheck; }
  };

  function renderCategorySelect() {
    const previous = $("categorySelect").value;
    $("categorySelect").innerHTML = `<option value="all">${esc(T().all_categories)}</option>` + categories.map(c => `<option value="${c}">${esc(categoryName(c))}</option>`).join("");
    $("categorySelect").value = categories.includes(previous) ? previous : "all";
  }
  function renderSelection() {
    $("selectionLine").textContent = `${selected.size} ${T().selected}`;
    $("optNext").disabled = selected.size === 0;
  }
  function renderOptions() {
    const filter = $("categorySelect").value || "all";
    const grid = $("cardGrid"); grid.innerHTML = "";
    CAT.tweaks.filter(t => filter === "all" || categoryOf(t) === filter).forEach(t => {
      const option = document.createElement("article"); option.className = "option";
      const kinds = Object.entries(t.kinds || {}).map(([kind,count]) => `${T().kinds[kind] || kind}: ${count}`).join(" · ");
      option.innerHTML = `<label class="option-main"><input type="checkbox" data-tweak="${esc(t.id)}" ${selected.has(t.id) ? "checked" : ""} /><span class="option-copy"><span class="option-title">${esc(nameOf(t))}</span><span class="option-description">${esc(t.description?.[lang] || "")}</span><span class="option-tags">${t.risk === "high" ? `<span class="tag risk">${T().risk_high}</span>` : t.risk === "medium" ? `<span class="tag">${T().risk_medium}</span>` : ""}${t.requires_reboot ? `<span class="tag">${T().reboot}</span>` : ""}</span></span></label><details class="details"><summary>${T().action_details}</summary><p>${t.actions_count} ${T().actions} · ${esc(kinds)}</p></details>`;
      option.querySelector("input").onchange = event => {
        event.target.checked ? selected.add(t.id) : selected.delete(t.id);
        $("profileSelect").value = "custom"; renderSelection();
      };
      grid.appendChild(option);
    });
    renderSelection();
  }
  $("categorySelect").onchange = renderOptions;
  $("profileSelect").onchange = event => {
    const preset = event.target.value;
    if (preset !== "custom") {
      selected.clear();
      CAT.tweaks.forEach(t => { if (preset === "balanced" ? t.risk === "low" && t.default : t.risk !== "high") selected.add(t.id); });
    }
    renderOptions();
  };
  function renderReview() {
    const choices = selectedTweaks();
    const high = choices.filter(t => t.risk === "high");
    const restart = choices.some(t => t.requires_reboot);
    $("reviewBody").innerHTML = `<div class="review-summary"><div class="summary-row"><span>${T().selection}</span><b>${choices.length}</b></div><div class="summary-row"><span>${T().restart_needed}</span><b>${restart ? T().yes : T().no}</b></div></div>${high.length ? `<p class="warning"><strong>${T().high_warn}</strong><br>${high.map(t => esc(nameOf(t))).join("، ")}</p>` : ""}<details class="details"><summary>${T().selected_details}</summary><ul class="review-list">${choices.map(t => `<li>${esc(nameOf(t))}</li>`).join("")}</ul></details>`;
    $("previewResult").textContent = "";
  }
  $("previewBtn").onclick = async () => {
    const button = $("previewBtn"); button.disabled = true; $("previewResult").textContent = T().preview_working;
    try {
      if (!TAURI) { $("previewResult").textContent = T().preview_app_only; return; }
      const counts = await invoke("preview_plan", {selected:[...selected]});
      $("previewResult").textContent = `${T().preview_changes}: ${counts.applied} · ${T().r_already}: ${counts.already} · ${T().r_na}: ${counts.not_applicable} · ${T().r_failed}: ${counts.failed}. ${T().preview_limit}`;
      if (counts.diagnostics?.length) window.AZ_DIAG.show(counts.diagnostics[0][2]);
    } catch (e) { $("previewResult").textContent = String(e); }
    finally { button.disabled = false; }
  };

  function setProgress(value, id) {
    const percent = Math.max(0,Math.min(100,Math.round(value)));
    $("pct").textContent = percent; $("progressFill").style.width = `${percent}%`;
    $("progressTrack").setAttribute("aria-valuenow",String(percent));
    if (id) $("statusLine").textContent = nameOf(CAT.tweaks.find(t => t.id === id)) || id;
  }
  async function runApply() {
    running = true; setProgress(0); $("statusLine").textContent = T().checking;
    let unlisten = null;
    try {
      if (TAURI) {
        unlisten = await window.__TAURI__.event.listen("apply-progress", ({payload:p}) => setProgress(p.done / Math.max(p.total,1) * 100,p.id));
        lastReport = await invoke("run_plan", {selected:[...selected],dryRun:false});
      } else {
        // Browser design preview only; it never invokes system operations.
        const choices = selectedTweaks();
        const counts = {applied:0,already:0,not_applicable:0,failed:0,skipped:0};
        for (let i=0;i<choices.length;i++) {
          const t = choices[i]; counts.applied += t.actions_count;
          setProgress((i+1)/choices.length*100,t.id);
          await new Promise(resolve => setTimeout(resolve,140));
        }
        lastReport = {counts,failures:[],archive:null};
      }
    } catch (e) {
      lastReport = {aborted:true,counts:null,failures:[["azizos",null,String(e)]],diagnostics:e.diagnostic ? [["azizos",null,e.diagnostic]] : [],archive:null};
    } finally {
      if (unlisten) { try { unlisten(); } catch (e) { void window.AZ_DIAG.record("progress_cleanup", e); } }
      running = false; setProgress(100); go("done");
    }
  }
  function renderDone() {
    const counts = lastReport?.counts || {applied:0,already:0,not_applicable:0,failed:0};
    const failures = lastReport?.failures || [];
    const renderFailure = (id, index, error) => {
      const diagnostic = lastReport?.diagnostics?.find(d => d[0] === id && d[1] === index)?.[2];
      const step = index == null ? T().operation : index > 1000000 ? "verify" : `#${index+1}`;
      return `<div class="failure-item"><b>${esc(nameOf(CAT.tweaks.find(t => t.id === id)) || id)} · ${esc(step)}</b>${diagnostic ? window.AZ_DIAG.html(diagnostic) : `<code>${esc(error)}</code>`}</div>`;
    };
    $("doneBody").innerHTML = `<h1 id="doneTitle">${lastReport?.aborted ? T().done_aborted : counts.failed ? T().done_some : T().done_ok}</h1><p class="result-message">${!TAURI ? T().finish_preview : lastReport?.aborted ? T().aborted_note : counts.failed ? T().done_errors : T().done_sub}</p><div class="result-overview" ${lastReport?.aborted ? "hidden" : ""}>${[[T().r_applied,counts.applied],[T().r_already,counts.already],[T().r_na,counts.not_applicable],[T().r_failed,counts.failed]].map(([label,count],i) => `<div class="summary-row ${i === 3 && count ? "has-errors" : ""}"><span>${label}</span><b>${count}</b></div>`).join("")}</div>${failures.length ? `<details class="details"><summary>${T().failures_title} (${failures.length})</summary>${failures.map(([id,index,error]) => renderFailure(id,index,error)).join("")}</details>` : ""}${lastReport?.archive ? `<details class="details"><summary>${T().archive_note}</summary><code class="archive-path">${esc(lastReport.archive)}</code></details>` : ""}`;
    $("finishBtn").textContent = $("autoRestart").checked && !lastReport?.aborted ? T().restart_now : T().finish;
    window.AZ_DIAG.showRepairs(lastReport?.log?.recoveries);
    if (lastReport?.log?.error) window.AZ_DIAG.show({message:`Logging error: ${lastReport.log.error}`});
  }
  $("finishBtn").onclick = async () => {
    const button = $("finishBtn"); button.disabled = true;
    try {
      if (!TAURI) { $("finishStatus").textContent = T().finish_preview; return; }
      const result = await invoke("finish", {autoRestart:$("autoRestart").checked && !lastReport?.aborted});
      if (!result.restarting) $("finishStatus").textContent = result.optimizer_scheduled ? T().finish_manual : T().finish_manual_no_opt;
    } catch (e) { $("finishStatus").textContent = String(e); }
    finally { button.disabled = false; }
  };
  window.addEventListener("azizos-app-repaired", () => {
    if (running) return;
    Promise.all([loadRequirements(),loadSystem()]).then(() => applyLanguage()).catch(e => window.AZ_DIAG.show(e));
  });
  applyLanguage();
  Promise.all([loadRequirements(),loadSystem()]).then(() => { applyLanguage(); go(current); });
})();
