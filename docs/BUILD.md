# البناء على جهازك (Windows)

بيئة كلود السحابية ما فيها linker/شاشة، فالبناء والتجربة يصيرون على جهازك. الخطوات:

## 1. الأدوات (مرة وحدة)

1. **Rust** — ثبّت من <https://rustup.rs> (يختار MSVC toolchain تلقائياً).
2. **Visual Studio Build Tools** (الـ linker) — ثبّت "Desktop development with C++" من
   <https://visualstudio.microsoft.com/visual-cpp-build-tools/>.
3. **Node.js LTS** (للواجهة Tauri) — <https://nodejs.org>.
4. **Tauri CLI**: `cargo install tauri-cli`.
5. **WebView2** — موجود أصلاً على ويندوز 11؛ على 10 ثبّت Evergreen Runtime.

تأكد:
```bash
rustc --version
cargo --version
node --version
```

## 2. بناء المحرّك وتشغيل الاختبارات (قلب المشروع)

```bash
cd engine
cargo test            # يشغّل اختبارات schema/catalog/exec/detect — لازم كلها تنجح
cargo run -- ../data/tweaks   # يحمّل الكتالوج ويطبع الخطة
cargo build --release # يبني المحرّك (بايناري صغير)
```

> هذي الخطوة تتحقق من كل المنطق المثبّت (التخطيط، عزل الأخطاء، التقييم) على جهازك.

## 3. بناء الباكند الحقيقي (Windows API)

ملفات `exec_windows.rs` و `detect_windows.rs` تُبنى تلقائياً مع `cargo build` على ويندوز
(هي خلف `#[cfg(windows)]`). لو تغيّرت توقيعات مكتبة `windows`، عدّل حسب رسالة المترجم.

## 4. بناء التطبيق بالواجهة

```bash
cd ui
npm install
cargo tauri build     # يطلّع Azizos.exe مثبّت واحد
```

## 5. التوقيع الرقمي (ضد الانتحال)

وقّع الناتج بشهادتك:
```bash
signtool sign /fd SHA256 /a /tr http://timestamp.digicert.com /td SHA256 "Azizos.exe"
```
- أي نسخة محقونة يطلع توقيعها مكسور/مجهول.
- حدّث `Version` في `playbook.conf` مع كل إصدار.

## 6. الأصول المدمجة (bundled/installer)

الأفعال `bundled` و `installer` تحتاج ملفاتها (باتشاتك، DevManView، OpenShell، VC++…)
مضمّنة داخل الـ exe كموارد. توضع في `engine/assets/` وتُدمج وقت البناء، وتُفك لمجلد مؤقت
وقت التشغيل ثم تُمسح. (نضيف مجلد `assets/` ومنطق الدمج مع الباكند.)
