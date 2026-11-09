# حل كشف النظام + فحص المتطلّبات — الخطوة 3

منطق التقييم في [`engine/src/detect.rs`](../engine/src/detect.rs) — **Rust نقي + مُختبَر**. القراءة الفعلية خلف واجهة `Detector` (الباكند `detect_windows.rs`, `#[cfg(windows)]`). **كل شيء قراءة فقط، لا يغيّر الجهاز.**

## كشف النظام (SystemProfile)

| الحقل | المصدر (Windows API) |
|---|---|
| `build` | `RtlGetVersion` أو ريجستري `...\CurrentVersion` → `CurrentBuildNumber` |
| `edition` | `CurrentVersion\EditionID` (Pro/Enterprise/IoTEnterprise/…) → نطابقه بـ ltsc/iot/pro |
| `device` laptop/desktop | `GetSystemPowerStatus` (وجود بطارية) + `Win32_SystemEnclosure.ChassisTypes` |
| `gpu` nvidia/amd/intel | `EnumDisplayDevices` / `SetupDi` → VendorID (0x10DE/0x1002/0x8086) |
| `disk` ssd/hdd | `IOCTL_STORAGE_QUERY_PROPERTY` (`DeviceSeekPenalty`) على قرص النظام |

يُغذّي `compat` في الكتالوج → التويك غير المتوافق **يُتخطّى** لا يفشل.

## المتطلّبات الثلاثة (Pre-flight)

> بدون إنترنت — التطبيق مكتفٍ ذاتياً.

| المتطلّب | الكشف (قراءة) | المعالجة في الواجهة |
|---|---|---|
| **DefenderToggled** | `defender_realtime_on`: ريجستري `...\Windows Defender\Real-Time Protection\DisableRealtimeMonitoring` + حالة Tamper (قراءة) | زر "عالجها تلقائياً" → موافقة صريحة → يشغّل سكربتك `DisableDefender.ps1`؛ أو "بنفسي" → يفتح Windows Security ثم يعيد الفحص |
| **NoThirdPartyAv** | `third_party_av`: WMI `root\SecurityCenter2 → AntiVirusProduct` (نستثني Defender) | إرشاد المستخدم يزيله بنفسه (لا نزيل مضاد أحد) |
| **PluggedIn** | `on_ac_power`: `GetSystemPowerStatus.ACLineStatus`؛ `None` = ديسكتوب | طلب توصيل الشاحن، ثم إعادة فحص |

منطق البوابة:
- `check_requirements(detector)` → قائمة حالات (Pass/Fail/NotApplicable).
- `all_satisfied()` → يسمح بالمتابعة (NotApplicable يُحتسب مُرضياً، مثل البطارية على ديسكتوب).

## حدود صريحة (ثابتة)
- لا نكتب كوداً جديداً "يلتف" على الحماية — نكشف الحالة ونعرض موافقة واضحة، والتعطيل يشغّل سكربتاتك الموجودة.
- التطبيق لا يزيل مضاد فيروسات المستخدم؛ يرشده فقط.
- الأوبتمايزر عندك فيه استرجاع الويندوز ديفولت كشبكة أمان.
