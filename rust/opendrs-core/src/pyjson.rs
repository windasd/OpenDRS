//! Python's built-in conversions applied to what `json.loads` returns (`str()`, `repr()`, `bool()`,
//! `float()`, iterating), for reading the JSON files the Python version also reads
//! (`settings.json`, the RadioID cache's `meta.json`) exactly as it does.

use serde_json::Value;

use crate::pystr::{float_repr, parse_float, str_repr};

/// `bool(value)`.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// `str(value)`.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        _ => py_repr(value),
    }
}

/// `repr(value)`.
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) if n.is_f64() => n.as_f64().map_or_else(String::new, float_repr),
        Value::Number(n) => n.to_string(),
        Value::String(s) => str_repr(s),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Object(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", str_repr(k), py_repr(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

/// `float(value)`; `None` where Python raises.
pub fn py_float(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => parse_float(s),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

/// `[str(item) for item in value]`; `None` where Python raises `TypeError` (not iterable).
pub fn py_str_items(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::Array(items) => Some(items.iter().map(py_str).collect()),
        Value::String(s) => Some(s.chars().map(String::from).collect()),
        Value::Object(map) => Some(map.keys().cloned().collect()),
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn python_conversions() {
        assert_eq!(py_str(&json!(null)), "None");
        assert_eq!(py_str(&json!(1.5e20)), "1.5e+20");
        assert_eq!(py_str(&json!(7)), "7");
        assert_eq!(
            py_str(&json!([1, "a'b", null, {"k": true}])),
            "[1, \"a'b\", None, {'k': True}]"
        );
        assert!(!truthy(&json!(0.0)) && truthy(&json!("x")) && !truthy(&json!({})));
        assert_eq!(py_float(&json!(" 1_000.5 ")), Some(1000.5));
        assert_eq!(py_float(&json!([1])), None);
        assert_eq!(
            py_str_items(&json!("ab")),
            Some(vec!["a".into(), "b".into()])
        );
        assert_eq!(py_str_items(&json!(3)), None);
    }
}
