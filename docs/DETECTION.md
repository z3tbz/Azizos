# كشف النظام وفحص المتطلّبات

التقييم في [`engine/src/detect.rs`](../engine/src/detect.rs) (نقي ومُختبَر)، والقراءة الفعلية في
[`detect_windows.rs`](../engine/src/detect_windows.rs). **كل شيء قراءة فقط — ما يغيّر الجهاز.**

## SystemProfile — أرخص مصدر أولاً

| الحقل | المصدر | التكلفة |
|---|---|---|
| `build` | ريجستري `CurrentVersion\CurrentBuildNumber` | ميكروثواني |
| `edition` | ريجستري `EditionID` → home / pro / education / enterprise / ltsc / iot | ميكروثواني |
| `device` | `GetSystemPowerStatus` (BatteryFlag 128 = بدون بطارية = مكتبي) | استدعاء API واحد |
| `gpu` | معرّف PCI من `MatchingDeviceId` في فئة كروت الشاشة: `10DE` NVIDIA · `1002/1022` AMD · `8086` Intel. الكرت المنفصل يغلب المدمج (لابتوب Intel+NVIDIA = NVIDIA) | ريجستري، مستقل عن لغة الجهاز واسم التسويق |
| `disk` | `DeviceIoControl` على قرص النظام: `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` ثم `IOCTL_STORAGE_QUERY_PROPERTY` (`StorageDeviceSeekPenaltyProperty`) | بدون WMI ولا PowerShell |

النتيجة تغذّي `compat` في الكتالوج: التويك غير المتوافق **يُتخطّى**، ما يفشل.

## المتطلّبات (Pre-flight)

| المتطلّب | الكشف (قراءة) | الحل في الواجهة |
|---|---|---|
| `SupportedBuild` | 19044 ≤ build ≤ 26300 | لا يوجد إصلاح تلقائي |
| `DefenderToggled` | ريجستري `Real-Time Protection\DisableRealtimeMonitoring` (الإعداد + السياسة) | «عالجها تلقائياً» = موافقة صريحة ثم تشغيل **سكربتك أنت** المربوط في `data/preflight.json`؛ أو «أسوّيها بنفسي» = فتح Windows Security ثم إعادة الفحص |
| `NoThirdPartyAv` | WMI `root/SecurityCenter2 → AntiVirusProduct` (استدعاء PowerShell مخفي واحد، ما فيه واجهة أخف) | إرشاد فقط + إعادة فحص |
| `PluggedIn` | `GetSystemPowerStatus.ACLineStatus`؛ المكتبي = غير منطبق | إرشاد + إعادة فحص |

## حدود ثابتة

- لا كود جديد يتجاوز الحماية؛ نكشف الحالة فقط، والإصلاح التلقائي يشغّل سكربت صاحب البلاي بوك بعد موافقة المستخدم.
- التطبيق ما يزيل مضاد فيروسات أحد.
