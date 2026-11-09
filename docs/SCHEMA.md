# Azizos Tweak Schema

كل تويك ملف JSON واحد في `data/tweaks/`. المحرّك يحمّلها كلها ويبني منها الواجهة والتنفيذ.

## الحقول

| الحقل | النوع | الوصف |
|---|---|---|
| `id` | string | معرّف فريد (kebab-case) |
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
{ "type":"registry", "op":"set|delete|delete_key|take_own",
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

// device  (enable/disable by friendly name)
{ "type":"device", "op":"disable|enable", "name":"Microsoft GS Wavetable Synth" }

// bcd
{ "type":"bcd", "args":"/set bootmenupolicy Legacy" }

// powerplan
{ "type":"powerplan", "import":"Azizos.pow", "guid":"..." }

// env  (environment variable)
{ "type":"env", "name":"DOTNET_CLI_TELEMETRY_OPTOUT", "data":"1", "scope":"machine|user" }
```

## أنواع الـ Checks (verify)

```jsonc
{ "type":"registry", "hive":"HKCU", "path":"...", "name":"...", "equals": 0 }
{ "type":"service",  "name":"DiagTrack", "startup":"disabled" }
{ "type":"file_absent", "path":"..." }
```

## التراجع (revert)

افتراضياً المحرّك يلتقط القيمة الأصلية قبل التطبيق ويخزّنها في سجل التراجع.
يمكن تخصيص تراجع يدوي بإضافة `"revert":[ ...actions... ]`.
