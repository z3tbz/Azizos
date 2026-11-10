# البناء على جهازك (Windows)

بيئة كلود السحابية ما فيها linker/شاشة، فالبناء والتجربة يصيرون على جهازك. الخطوات:

## 1. الأدوات (مرة وحدة)

1. **Rust** — ثبّت من <https://rustup.rs>.
2. **Visual Studio** (Build Tools يكفي، أو Community كاملة) مع workload
   "Desktop development with C++" — فيها الـ linker وWindows SDK اللي Rust يحتاجها
   على MSVC. تحميل: <https://visualstudio.microsoft.com/visual-cpp-build-tools/>.
3. تأكد إن توولتشين Rust النشط هو MSVC لا GNU (مهم — توولتشين GNU ما يقدر يربط
   مكتبات Tauri/windows-sys أصلاً):
   ```bash
   rustup toolchain install stable-x86_64-pc-windows-msvc
   rustup default stable-x86_64-pc-windows-msvc
   rustup show   # تأكد إن "active toolchain" ينتهي بـ -msvc
   ```
4. **Tauri CLI**: `cargo install tauri-cli --version "^2"` (ما يحتاج Node — الواجهة HTML/JS جاهزة).
5. **WebView2** — موجود أصلاً على ويندوز 11؛ على 10 ثبّت Evergreen Runtime.

تأكد:
```bash
rustc --version
cargo --version
cargo tauri --version
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
# 2) الأسماء والأوصاف العربية/الإنجليزية من scripts/option-names.json + فهرس الواجهة (ui/catalog.js)
powershell -File scripts/apply-names.ps1
# 3) انسخ Executables إلى assets (تُدمج داخل الـ exe)
powershell -File scripts/sync-assets.ps1 -PlaybookRoot <مجلد البلاي بوك>
# 4) ثلاثة ملفات كان البلاي بوك الأصلي يحمّلها من النت وقت التثبيت — Azizos يدمجها
#    بدل ما ينزّلها، فحملها مرة وحدة من مصادرها الرسمية (أو حدّث الروابط في
#    data/assets-needed.json لو تبغى نسخة أحدث):
powershell -File scripts/fetch-vendor-assets.ps1
```

| الملف | يحتاجه | المصدر الرسمي |
|---|---|---|
| `vcpp.exe` | core-commands | VisualCppRedist_AIO_x86_x64.exe (abbodi1406/vcredist على GitHub) |
| `OpenShell.exe` | openshell | Open-Shell-Menu v4.4.198 الرسمي على GitHub |
| `7zip.exe` | 7zip | مثبّت 7-Zip x64 الرسمي من 7-zip.org |

**مهم:** `app/build.rs` يدمج في الـ exe **فقط الملفات اللي تويك فعلياً يفتحها بالاسم** —
أي ملف ثاني تحطه في `assets/` (مواد خام لتويكات مستقبلية مثلاً) يبقى على القرص وما يدخل
الـ exe أبداً، فالبرنامج النهائي ما يحمل وزن ميت ولا ملفات تنفيذية غير مستخدمة. لو ملف
مطلوب ناقص، يطبع البناء تحذير واضح، وفي التشغيل التويك يسجّل "missing asset" بدل ما
يفشل بصمت.

## 4. الاختبارات ثم البناء

```bash
cargo test --workspace          # schema / catalog / exec / detect + الكتالوج المدمج
cd app && cargo tauri build      # target\release\azizos-app.exe + مثبّت NSIS
```

- الـ exe لحاله كافي (كل التويكات والأصول المستخدَمة فعلياً داخله؛ ~48MB أغلبها مثبّت
  vcpp.exe نفسه). يحتاج WebView2 (موجود
  في ويندوز 11، وفي 10 مع التحديثات). تويك إزالة Edge ما يمس WebView2.
- يطلب صلاحيات المسؤول تلقائياً (`app/azizos.manifest`). الأيقونة في `app/icons/icon.ico`.
- التقرير ينحفظ في `سطح المكتب\Azizos-Report-<الوقت>` (report.txt + report.json فيه القيم
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
