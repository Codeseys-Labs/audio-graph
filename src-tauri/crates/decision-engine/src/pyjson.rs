//! Python `json.dumps(value, ensure_ascii=False, ...)` text, byte for byte, for checkpoints whose prompts
//! embed JSON: d3 uses the default separators, Clef-flash compact separators with sorted keys.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// `json.dumps(v, ensure_ascii=False)`: separators `", "` / `": "`, keys in insertion order.
    Default,
    /// `json.dumps(v, ensure_ascii=False, separators=(",", ":"), sort_keys=True)`.
    CompactSorted,
}

pub fn dumps(value: &Value, style: Style) -> String {
    let mut out = String::new();
    write(value, style, &mut out);
    out
}

fn write(value: &Value, style: Style, out: &mut String) {
    let (item_sep, key_sep) = match style {
        Style::Default => (", ", ": "),
        Style::CompactSorted => (",", ":"),
    };
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                out.push_str(&float_repr(n.as_f64().unwrap_or(f64::NAN)));
            }
        }
        Value::String(s) => write_str(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(item_sep);
                }
                write(item, style, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            if style == Style::CompactSorted {
                // Python sorts str keys by code point; UTF-8 byte order is the same order.
                entries.sort_by(|a, b| a.0.cmp(b.0));
            }
            out.push('{');
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(item_sep);
                }
                write_str(k, out);
                out.push_str(key_sep);
                write(v, style, out);
            }
            out.push('}');
        }
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Python `repr(float)`: shortest round-trip digits, scientific below 1e-4 or from 1e16.
fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0" } else { "0.0" }.into();
    }
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if (-4..16).contains(&exp) {
        let plain = format!("{x}");
        if plain.contains('.') {
            plain
        } else {
            format!("{plain}.0")
        }
    } else {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exp.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_styles_match_python() {
        let v: Value = serde_json::from_str(
            r#"{"b": [1, 2.5, "x\ny"], "a": {"d": null, "c": true}, "é": 1e-7, "big": 1e16, "ctl": "\u0001"}"#,
        )
        .unwrap();
        assert_eq!(
            dumps(&v, Style::Default),
            r#"{"b": [1, 2.5, "x\ny"], "a": {"d": null, "c": true}, "é": 1e-07, "big": 1e+16, "ctl": "\u0001"}"#
        );
        assert_eq!(
            dumps(&v, Style::CompactSorted),
            r#"{"a":{"c":true,"d":null},"b":[1,2.5,"x\ny"],"big":1e+16,"ctl":"\u0001","é":1e-07}"#
        );
    }
}
