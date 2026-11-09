# المحرّك (Executor)

المنطق كله في [`engine/src/exec.rs`](../engine/src/exec.rs): **Rust نقي ومُختبَر** يشتغل ويُختبر على أي نظام.
كل لمسة لويندوز خلف واجهة واحدة `Backend` (ثلاث دوال)، والتنفيذ الحقيقي في
[`exec_windows.rs`](../engine/src/exec_windows.rs) و[`win32.rs`](../engine/src/win32.rs) (ميزة `native`).

```text
UI (ui/app.js) ──invoke──► app/src/main.rs ──► exec::run_phase_observed ──► Backend
                                                     │                     ├─ snapshot()  قراءة القيمة الأصلية
                                                     │                     ├─ apply()     Done / Already / NotApplicable / Err
                                                     │                     └─ verify()    إعادة القراءة
                                                     └─► Report → تقرير سطح المكتب (txt + json)
```

## 1. نتيجة كل إجراء (صادقة، بدون أخطاء وهمية)

| الحالة | متى | يُعدّ خطأ؟ |
|---|---|---|
| `Applied` / `Verified` | تغيّر شيء فعلاً (وتأكّد بالقراءة إن وُجد `verify`) | لا |
| `AlreadyApplied` | القيمة مطابقة أصلاً، المفتاح/الملف محذوف أصلاً، الخدمة متوقفة، المهمة معطّلة، التطبيق غير مثبّت | لا |
| `NotApplicable` | العنصر غير موجود في نسخة ويندوز هذي (خدمة/مهمة/ملف/عملية) | لا |
| `Failed` | رفض حقيقي من ويندوز، مع **رمز الخطأ** (مثل `exit code 5 (0x00000005) Access is denied`) | **نعم** |
| `Skipped` | وضع المعاينة (dry-run) أو تويك غير متوافق مع النسخة | لا |

`Report::failures()` يرجّع **الأخطاء الفعلية فقط**؛ الواجهة والتقرير يعرضون الأربع أرقام منفصلة.

## 2. المتانة

- **عزل كامل:** كل إجراء داخل `catch_unwind`؛ خطأ أو حتى panic في إجراء = سطر في التقرير والباقي يكمل.
- **اللقطة قبل التغيير:** `snapshot()` تقرأ القيمة الأصلية (النوع + البيانات بالضبط) قبل الكتابة، وتُحفظ في `report.json`
  كإجراء عكسي (`revert`) — تُحفظ فقط لما يتغيّر شيء فعلاً.
- **قراءة خفيفة ومباشرة:** اللقطة والفحص المسبق = استدعاء `RegQueryValueExW` داخل العملية نفسها (ميكروثواني)، بدون أي عملية خارجية.
- **مرحلتان:** الفوري أولاً، ثم ما يحتاج إعادة تشغيل (خدمات/BCD) آخر شي.
- **إعادة التشغيل مرتين آمنة (idempotent):** التشغيل الثاني يطلع كله «مطبّق مسبقاً».

## 3. أخف طبقة لكل نوع

| الإجراء | الطبقة | ليش هذي الأخف |
|---|---|---|
| `registry` set/delete/delete_key | `windows-registry` (غلاف Microsoft الرقيق فوق `Reg*W`) | داخل العملية، بدون `reg.exe`، النوع الصحيح (DWORD/QWORD/SZ/EXPAND/MULTI/BINARY) |
| `service` change | قيمة `Start` (0..4) في `HKLM\SYSTEM\CurrentControlSet\Services\<name>` | نفس ما يكتبه `sc config` بالضبط، بدون عملية؛ خدمة غير موجودة = غير منطبق (ما ننشئ مفتاح) |
| `service` stop / delete | SCM API: `OpenSCManagerW` → `ControlService` / `DeleteService` (`win32.rs`) | بدون `sc.exe`؛ رموز 1060/1062/1072 تُترجم لـ غير منطبق/مطبّق |
| `kill` | Toolhelp: `CreateToolhelp32Snapshot` + `TerminateProcess` | بدون `taskkill.exe`؛ يتجاهل عمليته هو والـ PID 0 |
| `file` delete/rename/copy | `std::fs` | داخل العملية |
| `env` | ريجستري `Environment` (machine/user) | نفس مصدر ويندوز |
| `scheduled_task` | فحص مسبق من `System32\Tasks\…` (XML) ثم `schtasks.exe` فقط إذا يلزم | غالباً بدون عملية إطلاقاً |
| `appx` | فحص مسبق من الريجستري (`PackageRepository` + `AppxAllUserStore`) ثم PowerShell فقط للموجود | تطبيق غير مثبّت = صفر عمليات |
| `bcd` | `bcdedit.exe` | ما فيه API عام أخف (BCD WMI أثقل) |
| `powerplan` | `powercfg.exe` | (مخطّط: `PowerWriteACValueIndex` من powrprof — انظر الخطة) |
| `command` / `bundled` / `installer` | عملية فرعية | سكربتات البلاي بوك نفسها |

**قواعد أي عملية فرعية (كلها في `WindowsBackend::proc`):**
- `CREATE_NO_WINDOW` → **ما تطلع أي نافذة سوداء**.
- أداة النظام تُحلّ لمسارها الكامل في `%SystemRoot%\System32` (`sys_tool`) — ما نبحث في PATH، فما أحد يقدر يزرع `bcdedit.exe` مزيّف.
- `System32` أول الـ PATH للعمليات الفرعية، ومجلد العمل = مجلد الأصول.
- PowerShell عبر `-EncodedCommand` (UTF-16 base64) → بدون مشاكل اقتباس أو ترميز عربي.
- الوسائط تمر كما هي (`raw_arg`) لأن `cmd` له قواعد اقتباس خاصة.
- مهلة زمنية لكل نوع (5/20/30 دقيقة) + قراءة stdout/stderr في خيوط منفصلة (ما تعلق على pipe ممتلئ).
- ملف مفقود من الأصول = رسالة واضحة `missing asset 'vcpp.exe'` بدل «is not recognized» المترجمة.

## 4. الجسر Rust ↔ التويكات

- التويكات JSON في `data/tweaks/` → `app/build.rs` يدمجها داخل الـ exe (`include_str!`) → تُفكّ مرة واحدة إلى `Catalog` عند التشغيل.
- الأصول (سكربتات/مثبّتات) مدمجة `include_bytes!` وتُفكّ عند الحاجة فقط إلى `%SystemRoot%\Temp\Azizos-<pid>-<nanos>` وتُحذف عند الإنهاء.
- الواجهة تستلم كتالوج خفيف (`ui/catalog.js` ~9KB: الاسم، الوصف، الخطورة، عدد الإجراءات وأنواعها) — الإجراءات الكاملة تبقى في المحرّك.
- الواجهة ↔ المحرّك: أوامر Tauri غير متزامنة (`spawn_blocking`)، والتقدّم حدث `apply-progress` لكل تويك مع حالته.

## 5. الاختبارات

```bash
cargo test -p azizos-engine --no-default-features   # المنطق النقي + الكتالوج الحقيقي (25 تويك × 6 نسخ)
cargo test -p azizos-engine                          # + اختبارات ويندوز الحقيقية (تحتاج MSVC Build Tools)
```

اختبارات ويندوز الحقيقية آمنة: قراءة فقط، أو مفتاح تجربة مؤقت تحت `HKCU\Software\AzizosEngineTest` يُحذف بعدها.
