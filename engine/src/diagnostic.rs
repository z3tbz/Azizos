//! Stable Azizos support codes are distinct from original Windows/exit codes.
//! Classification is advisory; the original message is always retained.
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub code: &'static str,
    pub category: &'static str,
    pub stage: String,
    pub native_code: Option<String>,
    pub message: String,
    pub solution_ar: &'static str,
    pub solution_en: &'static str,
}

fn original_code(message: &str) -> Option<String> {
    // Only parse explicit code markers; never reinterpret an arbitrary number
    // in a filename/action index as a Windows error.
    let lower = message.to_ascii_lowercase();
    // Windows error Display uses `(0x...)`; explicit HRESULT/code markers are
    // also accepted. A hex-looking segment inside a path is not an error code.
    let hex_start = lower.match_indices("0x").find_map(|(start, _)| {
        let prefix = &lower[..start];
        let before = prefix.chars().next_back();
        (before == Some('(') || prefix.ends_with("hresult: ") || prefix.ends_with("hresult ")
            || prefix.ends_with("code: ") || prefix.ends_with("code ")).then_some(start)
    });
    if let Some(start) = hex_start {
        let end = message[start + 2..].find(|c: char| !c.is_ascii_hexdigit())
            .map(|n| start + 2 + n).unwrap_or(message.len());
        if end > start + 2 { return Some(message[start..end].to_ascii_uppercase().replacen("0X", "0x", 1)); }
    }
    for marker in ["os error ", "win32 error ", "exit code ", "error code "] {
        if let Some(start) = lower.find(marker).map(|n| n + marker.len()) {
            let token: String = message[start..].chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
            if !token.is_empty() && token != "-" { return Some(token); }
        }
    }
    None
}

pub fn classify(stage: &str, message: &str) -> Diagnostic {
    let lower = message.to_ascii_lowercase();
    let native = original_code(message);
    // Exit codes belong to their program, even if a numeric value matches a
    // Win32 constant. Only identify HRESULT_FROM_WIN32 or explicit OS codes.
    let is_exit = lower.contains("exit code ");
    let has_os = |decimal: &str, hex: &str| !is_exit &&
        (native.as_deref() == Some(hex) || native.as_deref() == Some(decimal));
    let (code, category, ar, en) = if stage.contains("panic") || lower.contains("internal error:") || lower.contains("snapshot panic:") {
        ("AZ-1900", "internal", "أوقف إعادة المحاولة وأرسل السجل للمطور مع خطوات حدوث المشكلة.", "Send the log and reproduction steps to the developer before retrying.")
    } else if has_os("5", "0x80070005") || lower.contains("access is denied") || lower.contains("access denied") {
        ("AZ-1001", "permission", "تحقق من تشغيل التطبيق كمسؤول وصلاحيات الهدف؛ لا تغيّر حماية النظام عشوائياً.", "Check elevation and target permissions; do not indiscriminately change system protection.")
    } else if native.as_deref() == Some("0x80040154") {
        ("AZ-1003", "windows_component", "واجهة COM المطلوبة غير مسجلة. افحص توفر مكوّن ويندوز وخدمته في إصدارك؛ لا تعتبر الفحص ناجحاً.", "The requested COM class is not registered. Check component availability on this Windows edition; do not assume the probe passed.")
    } else if has_os("112", "0x80070070") || lower.contains("not enough space") || lower.contains("disk full") {
        ("AZ-1004", "storage", "وفّر مساحة في قرص النظام ومجلد التقرير، ثم أعد المحاولة بعد مراجعة حالة التنفيذ.", "Free space on the system/report drive and review run state before retrying.")
    } else if lower.contains("timed out") || lower.contains("timeout") {
        ("AZ-1005", "timeout", "راجع العملية والسجل؛ قد تكون بعض التغييرات نُفذت قبل انتهاء المهلة. لا تكرر التنفيذ مباشرة.", "Review process and log state; changes may have occurred before the timeout. Do not immediately rerun.")
    } else if lower.contains("unsupported") || lower.contains("refusing") || lower.contains("requires the author's original tool") {
        ("AZ-1006", "unsupported", "هذا الإجراء أو سياق التشغيل غير مدعوم حالياً. أرسل رقم التويك والإجراء للمطور لمطابقته مع البلاي بوك.", "Send the tweak/action reference to the developer to implement its exact playbook semantics.")
    } else if stage.contains("verify") || lower.starts_with("verify:") || lower.starts_with("verify error:") {
        ("AZ-1007", "verification", "لم يثبت الفحص تطبيق التغيير. راجع القيمة الفعلية ومتطلبات إعادة التشغيل قبل تكراره.", "Verify actual state and reboot requirements before applying the change again.")
    } else if lower.contains("missing asset") || has_os("2", "0x80070002") || has_os("3", "0x80070003") {
        ("AZ-1002", "missing_resource", "تحقق من الملف والمسار والملفات المدمجة في النسخة؛ أرسل السجل إذا كان ملف البلاي بوك مفقوداً.", "Check paths and bundled resources; report missing playbook assets with the log.")
    } else if stage.contains("snapshot") || lower.contains("original state could not be saved") || lower.contains("journal") {
        ("AZ-1008", "backup", "لم يكتمل حفظ الحالة الأصلية. تحقق من صلاحيات ومساحة مجلد التقرير قبل التطبيق.", "Original state was not safely saved. Check report directory permissions and free space.")
    } else if is_exit {
        ("AZ-1009", "child_process", "راجع اسم الأداة ورمز خروجها ونص الخطأ الأصلي. قد يكون التغيير جزئياً؛ لا تعِد تشغيل الخطة كاملة تلقائياً.", "Review the tool, exit code and original output. Changes may be partial; do not automatically rerun the entire plan.")
    } else if lower.contains("already running") {
        ("AZ-1010", "busy", "انتظر انتهاء العملية الحالية؛ لا تشغّل نسختين من التطبيق على نفس التغييرات.", "Wait for the current operation; avoid concurrent runs on the same targets.")
    } else if stage.contains("catalog") || lower.contains("invalid") || lower.contains("unknown tweak") {
        ("AZ-1011", "configuration", "راجع تعريف التويك والاختيارات وصيغة البيانات؛ لا تطبق تعريفاً غير صالح.", "Check tweak definitions, selection and data format before applying.")
    } else if stage.contains("precondition") {
        ("AZ-1201", "precondition", "راجع المتطلب المعروض ثم أعد الفحص. عدم تحقق المتطلب ليس دليلاً على عطل بالتطبيق.", "Resolve the displayed prerequisite and recheck; an unmet prerequisite is not itself an application fault.")
    } else {
        ("AZ-1099", "unclassified", "السبب غير محسوم. احتفظ بالسجل وأرسل تفاصيل الخطوة للمطور؛ لا تفترض أن إعادة التنفيذ آمنة.", "The cause is undetermined. Send the log and step details; do not assume rerunning is safe.")
    };
    Diagnostic { code, category, stage: stage.into(), native_code: native, message: message.into(), solution_ar: ar, solution_en: en }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_native_hresult_and_classifies_registration_failure() {
        let d = classify("preflight", "WSCProductList: Class not registered (0x80040154)");
        assert_eq!(d.code, "AZ-1003");
        assert_eq!(d.native_code.as_deref(), Some("0x80040154"));
    }
    #[test]
    fn support_code_does_not_replace_native_code_or_original_message() {
        let message = "C:\\file: Access is denied. (os error 5)";
        let d = classify("apply", message);
        assert_eq!(d.code, "AZ-1001");
        assert_eq!(d.native_code.as_deref(), Some("5"));
        assert_eq!(d.message, message);
    }
    #[test]
    fn exit_codes_and_unrelated_numbers_are_not_guessed_as_windows_errors() {
        assert_eq!(classify("apply", "tool: exit code 5 (0x00000005)").code, "AZ-1009");
        assert_eq!(classify("apply", "step 5 failed in C:\\42").native_code, None);
        assert_eq!(classify("apply", "failed in C:\\0x80070005.txt").native_code, None);
        assert_eq!(classify("apply", "unknown").code, "AZ-1099");
    }
    #[test]
    fn snapshot_and_timeout_have_safe_guidance() {
        assert_eq!(classify("snapshot", "journal write failed").code, "AZ-1008");
        assert_eq!(classify("apply", "tool timed out after 300s").code, "AZ-1005");
        assert_eq!(classify("panic", "unexpected").code, "AZ-1900");
    }
}
