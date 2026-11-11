# Azizos Tweak Schema

كل تويك ملف JSON واحد في `data/tweaks/`. المحرّك يحمّلها كلها ويبني منها الواجهة والتنفيذ.

## الحقول

| الحقل | النوع | الوصف |
|---|---|---|
| `id` | string | معرّف فريد (kebab-case) |
| `order` | number | أولوية ترتيب البلاي بوك؛ الافتراضي 0، والتبعيات تتقدّم عليها |
| `name` | {ar,en} | الاسم المعروض |
| `description` | {ar,en} | شرح مختصر للمستخدم |
| `category` | string | تصنيف (appearance/privacy/performance/network/services/debloat/security) |
| `profiles` | string[] | أي نمط يتبع له (balanced/gaming)؛ فارغ = متاح للكل |
| `default` | bool | مُفعّل افتراضياً في الواجهة |
| `requires_reboot` | bool | يحتاج إعادة تشغيل ليفعل |
| `risk` | string | low/medium/high — يُعرض تحذير للعالي |
| `compat` | object | بوابة التوافق (تحت) |
| `depends_on` | string[] | تويكات لازمة قبله |
| `conflicts_with` | string[] | تويكات تتعارض معه |
| `actions` | Action[] | ما يُنفّذ |
| `verify` | Check[] | كيف نتأكد أنه طُبّق |

### compat

```json
{ "builds": {"min": 19044, "max": 26300},
  "editions": ["any"],          // or ["ltsc","iot","pro",...]
  "device": "any",              // any | laptop | desktop
  "gpu": "any",                 // any | nvidia | amd | intel
  "disk": "any" }               // any | ssd | hdd
```

## أنواع الـ Actions

```jsonc
// registry
{ "type":"registry", "op":"set|delete|delete_key",
  "hive":"HKLM|HKCU|HKCR|HKU", "path":"Software\\...",
  "name":"ValueName", "kind":"dword|qword|sz|expand_sz|multi_sz|binary",
  "data": 0 }

// service
{ "type":"service", "name":"DiagTrack", "startup":"boot|system|auto|demand|disabled",
  "op":"change|delete|stop" }

// scheduled_task
{ "type":"scheduled_task", "path":"\\Microsoft\\...", "op":"disable|enable|delete" }

// appx
{ "type":"appx", "name":"*Spotify*", "scope":"family|app",
  "op":"remove|unregister|clear_cache" }

// file
{ "type":"file", "op":"delete|copy|rename", "path":"%windir%\\...", "to":"..." }

// command  (cmd.exe)
{ "type":"command", "exe":"bcdedit", "args":"/set disabledynamictick yes" }

// powershell
{ "type":"powershell", "script":"name.ps1", "runas":"user_elevated|trusted_installer" }

// kill: path_contains يمنع قتل عمليات أخرى لها الاسم نفسه
{ "type":"kill", "name":"setup", "path_contains":"\\Edge" }

// device  (enable/disable by friendly name)
{ "type":"device", "op":"disable|enable", "name":"Microsoft GS Wavetable Synth" }

// bcd
{ "type":"bcd", "args":"/set bootmenupolicy Legacy" }

// powerplan
{ "type":"powerplan", "import":"Azizos.pow", "guid":"..." }

// power_profile: ترجمة فرعي powerplan.bat الأصليين إلى powrprof
{ "type":"power_profile", "desktop_import":"Azizos.pow" }

// env  (environment variable)
{ "type":"env", "name":"DOTNET_CLI_TELEMETRY_OPTOUT", "data":"1", "scope":"machine|user" }
```

`powerplan.guid` حالياً مؤشر لتفعيل الخطة المستوردة، وليس ضماناً لاستيرادها بنفس المعرف.
`trusted_installer` قابل للقراءة في البيانات، لكن التنفيذ يرفضه صراحة؛ لا يشغّله بهوية مختلفة بصمت.
Appx `unregister` أو نطاق غير `family` يتطلب أداة المؤلف عند وجود الهدف؛ لا يتحول لإزالة كاملة.
مسح الكاش مباشر ويحافظ على SettingsCache.txt. الخدمة ذات wildcard تدعم الحذف/الإيقاف؛ تغيير بدء خدمات متعددة يتطلب إجراءات منفصلة لالتقاط أصولها.

### `ignore_errors` (على أي إجراء)

```jsonc
{ "type":"kill", "name":"msedge", "ignore_errors": true }
```

يقابل `ignoreErrors: true` و`errorAction: Ignore` في AME: المؤلف يقول إن فشل هذي الخطوة غير مهم.
لو فشلت، تُسجَّل في التقرير بسببها ورمزها كـ«غير منطبق» وما تُحسب خطأ. المحوّل يكتبها تلقائياً.

## أنواع الـ Checks (verify)

```jsonc
{ "type":"registry", "hive":"HKCU", "path":"...", "name":"...", "equals": 0 }
{ "type":"service",  "name":"DiagTrack", "startup":"disabled" }
{ "type":"file_absent", "path":"..." }
```

## التراجع (revert)

يلتقط المحرّك القيم الأصلية المدعومة ويكتبها قبل التطبيق في journal.jsonl، ثم يضيف العكس للتقرير عند نجاح التغيير.
حقل `revert` يُقرأ من البيانات لكنه لا يُطبَّق تلقائياً. الإجراءات بلا عكس آلي تظهر في without_automatic_restore.
راجع [حدود الاسترجاع](./AUDIT-2026-11-10.md).
