# البناء على جهازك (Windows)

بيئة كلود السحابية ما فيها linker/شاشة، فالبناء والتجربة يصيرون على جهازك. الخطوات:

## 1. الأدوات (مرة وحدة)

1. **Rust** — ثبّت من <https://rustup.rs> (يختار MSVC toolchain تلقائياً).
2. **Visual Studio Build Tools** (الـ linker) — ثبّت "Desktop development with C++" من
   <https://visualstudio.microsoft.com/visual-cpp-build-tools/>.
3. **Tauri CLI**: `cargo install tauri-cli --version "^2"` (ما يحتاج Node — الواجهة HTML/JS جاهزة).
4. **WebView2** — موجود أصلاً على ويندوز 11؛ على 10 ثبّت Evergreen Runtime.

تأكد:
```bash
rustc --version
cargo --version
```

## 2. بناء المحرّك وتشغيل الاختبارات (قلب المشروع)

```bash
cd engine
cargo test            # يشغّل اختبارات schema/catalog/exec/detect — لازم كلها تنجح
cargo run -- ../data/tweaks   # يحمّل الكتالوج ويطبع الخطة
cargo build --release # يبني المحرّك (بايناري صغير)
```

> هذي الخطوة تتحقق من كل المنطق المثبّت (التخطيط، عزل الأخطاء، التقييم) على جهازك.

## 3. تجهيز البيانات والأصول (قبل كل إصدار)

```powershell
# 1) حوّل البلاي بوك لصيغة Azizos (data/tweaks)
powershell -File scripts/convert-playbook.ps1 -PlaybookRoot <مجلد البلاي بوك> -OutDir data/tweaks
# 2) فهرس الواجهة الخفيف (ui/catalog.js)
powershell -File scripts/build-ui-catalog.ps1
# 3) انسخ Executables إلى assets (تُدمج داخل الـ exe)
powershell -File scripts/sync-assets.ps1 -PlaybookRoot <مجلد البلاي بوك>
```

`sync-assets` يطبع الملفات الناقصة. البلاي بوك الأصلي كان **يحمّلها من النت** وقت التثبيت،
وAzizos بدون نت، فلازم تحطها بنفسك في `assets/` بنفس الاسم:

| الملف | يحتاجه | المصدر الأصلي |
|---|---|---|
| `vcpp.exe` | core-commands | VisualCppRedist_AIO_x86_x64.exe (abbodi1406) |
| `OpenShell.exe` | openshell | OpenShellSetup_4_4_198.exe |
| `7zip.exe` | 7zip | مثبّت 7-Zip x64 (يتثبت بـ `/S`) |

إذا نقص ملف: التويك ما يوقف التشغيل — ينكتب في التقرير "missing asset".

## 4. الاختبارات ثم البناء

```bash
cargo test --workspace          # schema / catalog / exec / detect + الكتالوج المدمج
cd app && cargo tauri build      # targeteleasezizos-app.exe + مثبّت NSIS
```

- الـ exe لحاله كافي (كل التويكات والأصول داخله؛ 60MB تقريباً). يحتاج WebView2 (موجود
  في ويندوز 11، وفي 10 مع التحديثات). تويك إزالة Edge ما يمس WebView2.
- يطلب صلاحيات المسؤول تلقائياً (`app/azizos.manifest`). الأيقونة في `app/icons/icon.ico`.
- التقرير ينحفظ في `سطح المكتبAzizos-Report-<الوقت>` (report.txt + report.json فيه القيم
  السابقة للرجوع).
- بعد الريستارت يفتح AZIZ Gaming Optimizer مرة وحدة (RunOnce).

## 5. التوقيع الرقمي (ضد الانتحال)

وقّع الناتج بشهادتك:
```bash
signtool sign /fd SHA256 /a /tr http://timestamp.digicert.com /td SHA256 "Azizos.exe"
```
- أي نسخة محقونة يطلع توقيعها مكسور/مجهول.
- حدّث `Version` في `playbook.conf` مع كل إصدار.

## 6. "عالجها تلقائياً" في المتطلبات

في `data/preflight.json` حط اسم السكربت حقك (من `assets`) لكل متطلب، مثال:
`{"fixes": {"DefenderToggled": {"file": "<اسم سكربتك>", "args": ""}}}`.
التطبيق يشغّله **بعد موافقة المستخدم** فقط. إذا ما حطيت شي، الزر يفتح إعدادات الحماية
للمستخدم يسويها بنفسه.
