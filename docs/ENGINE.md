# حل التنفيذ (Executor) — الخطوة 2

المنطق المقاوم للأخطاء مكتوب في [`engine/src/exec.rs`](../engine/src/exec.rs) وهو **Rust نقي + مُختبَر** (عزل الفشل، التحقق، التراجع، المراحل، التقرير). كل استدعاءات ويندوز خلف واجهة `Backend`، فالباكند الحقيقي (`exec_windows.rs`, `#[cfg(windows)]`) يُبنى على جهازك.

## لماذا هذا التصميم أضمن
- الجزء الخطير (FFI مع ويندوز) معزول وصغير وكل نقطة فيه مراجَعة.
- المنطق المهم (اللي يمنع الكراش والتعارض) مثبّت باختبارات تشتغل على أي جهاز.
- `apply` لكل فعل معزول: الفشل يُسجَّل والتنفيذ يكمل — نفس متانة أمي وزرد، بنتائج منظّمة.

## خريطة كل فعل → Windows API (الباكند الحقيقي)

| الفعل | واجهة ويندوز | ملاحظات |
|---|---|---|
| `registry` set/delete | `windows-registry` crate (`RegSetValueEx`/`RegDeleteValue`) | كتابة مباشرة بالنوع الصحيح، بدون `reg.exe` |
| `registry` take_own | `RegSetKeySecurity` + تمكين `SeTakeOwnership` | للمفاتيح المحمية |
| `service` change startup | `OpenSCManager`→`OpenService`→`ChangeServiceConfig` | بدل `sc.exe`؛ 0..4 = boot..disabled |
| `service` delete/stop | `DeleteService` / `ControlService` | |
| `scheduled_task` | `ITaskService` COM | enable/disable/delete |
| `appx` | `PackageManager` (WinRT) أو `Remove-AppxPackage` fallback | family/app، unregister، clearCache |
| `file` | `std::fs` + `MoveFileEx` للمحمية | delete/copy/rename |
| `command` | `CreateProcess` | DISM/fsutil/label/rundll32 — أدوات نظام |
| `bundled` | فكّ الأصل المدمج لمجلد مؤقت ثم `CreateProcess` | .ps1→PowerShell، .bat→cmd، .exe→مباشر |
| `installer` | تشغيل المثبّت المدمج صامتاً | offline، بدون تنزيل |
| `bcd` | `bcdedit` عبر `CreateProcess` (أو BCD WMI) | |
| `powerplan` | `PowerImportPowerScheme` + `PowerSetActiveScheme` | |
| `device` | SetupAPI (`SetupDiSetClassInstallParams`) | enable/disable |
| `env` | `SetEnvironmentVariable` + كتابة الريجستري للدوام | machine/user |
| `powershell` (inline) | `CreateProcess powershell -Command` | الأوامر المضمّنة فقط |

## الصلاحيات
- التطبيق `requireAdministrator`.
- الأفعال التي تحتاج `TrustedInstaller` (مفاتيح/خدمات محمية): تُنفَّذ بانتحال رمز TrustedInstaller أو عبر أداة مضمّنة — يُحدَّد لكل فعل في الباكند.

## التوازي والمراحل
- `run_phase(Immediate)` ثم ريستارت ثم `run_phase(Deferred)`.
- الأفعال المستقلة (registry غالباً) تتوازى؛ `Action::is_serial()` = true للـ bcd/device/bundled/installer/command/powershell → تُسلسَل.

## التراجع والتقرير
- قبل كل فعل: `backend.snapshot(action)` يلتقط فعلاً عكسياً → سجل تراجع على القرص + Restore Point.
- بعد التطبيق: `verify` يعيد القراءة ويصنّف (Applied/Verified/Failed).
- النهاية: `Report.failures()` → تقرير ثنائي اللغة + أرشيف على الديسكتوب.
