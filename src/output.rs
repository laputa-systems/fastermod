//! Machine-readable and human-readable result rendering.

use crate::plan::EditPlan;

pub(crate) fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn json_string(text: &str) -> String {
    format!("\"{}\"", json_escape(text))
}

fn json_path(path: &std::path::Path) -> String {
    json_string(&path.to_string_lossy())
}

/// Serialize an edit plan as the frozen JSON result schema v1
/// (docs/refactor/cli.md). The string ends with a newline.
pub(crate) fn plan_json_report(plan: &EditPlan) -> String {
    let mut out = String::new();
    out.push_str("{\"schema\":1,\"mode\":");
    out.push_str(&json_string(&plan.mode));
    out.push_str(&format!(",\"applied\":{}", plan.applied));
    out.push_str(",\"operation_id\":");
    out.push_str(&json_string(&plan.operation_id));
    out.push_str(",\"coverage\":{\"roots\":[");
    for (i, root) in plan.coverage.roots.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json_string(root));
    }
    out.push_str(&format!(
        "],\"files_scanned\":{},\"files_edited\":{},\"files_skipped\":[",
        plan.coverage.files_scanned, plan.coverage.files_edited
    ));
    for (i, skipped) in plan.coverage.files_skipped.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"path\":{},\"reason\":{}}}",
            json_path(&skipped.path),
            json_string(&skipped.reason)
        ));
    }
    out.push_str("],\"unresolved\":[");
    for (i, unresolved) in plan.coverage.unresolved.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"path\":{},\"detail\":{}}}",
            json_path(&unresolved.path),
            json_string(&unresolved.detail)
        ));
    }
    out.push_str("]},\"edits\":[");
    let mut first = true;
    for file in &plan.files {
        for edit in &file.edits {
            if !first {
                out.push(',');
            }
            first = false;
            out.push('{');
            out.push_str("\"path\":");
            out.push_str(&json_path(&file.path));
            out.push_str(&format!(
                ",\"byte_start\":{},\"byte_end\":{},\"applied\":{},\"anchor\":{{\"line\":{},\"column\":{},\"excerpt\":{}}},\"kind\":{}",
                edit.byte_start,
                edit.byte_end,
                file.applied,
                edit.anchor.line,
                edit.anchor.column,
                json_string(&edit.anchor.excerpt),
                json_string(edit.kind.as_str())
            ));
            out.push('}');
        }
    }
    out.push_str("],\"diagnostics\":[");
    for (i, diagnostic) in plan.diagnostics.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"severity\":{},\"code\":{},\"message\":{}",
            json_string(diagnostic.severity.as_str()),
            json_string(&diagnostic.code),
            json_string(&diagnostic.message)
        ));
        if let Some(path) = &diagnostic.path {
            out.push_str(",\"path\":");
            out.push_str(&json_path(path));
        }
        out.push('}');
    }
    out.push_str("]}\n");
    out
}

pub(crate) fn languages_report(json: bool) -> String {
    if json {
        return "{\"schema\":1,\"modes\":{\"text\":{\"status\":\"available\"},\"ident\":{\"status\":\"unavailable\",\"ticket\":\"REFAC-6\"},\"rename\":{\"status\":\"unavailable\",\"ticket\":\"REFAC-7\"},\"rewrite\":{\"status\":\"unavailable\",\"ticket\":\"REFAC-6\"},\"rules\":{\"status\":\"unavailable\",\"ticket\":\"REFAC-12\"},\"apply-plan\":{\"status\":\"unavailable\",\"ticket\":\"REFAC-12\"}},\"languages\":[]}\n".to_string();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "fastermod {} capability report\n",
        env!("CARGO_PKG_VERSION")
    ));
    out.push_str("modes:\n");
    out.push_str("  text       available (regex/literal substitution)\n");
    out.push_str("  ident      unavailable (REFAC-6)\n");
    out.push_str("  rename     unavailable (REFAC-7/8)\n");
    out.push_str("  rewrite    unavailable (REFAC-6)\n");
    out.push_str("  rules      unavailable (REFAC-12)\n");
    out.push_str("  apply-plan unavailable (REFAC-12)\n");
    out.push_str("languages: no syntax-aware adapters yet; the typed capability\n");
    out.push_str("registry lands with REFAC-6. Parser availability is not rename\n");
    out.push_str("support.\n");
    out
}

