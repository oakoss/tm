//! Field changes as the writing commands build them from their flags.

use serde_json::{Value, json};
use tm_core::Change;
use tm_core::scheme::{FieldType, Scheme};

/// `FIELD=VALUE`, split at the first `=`.
pub(crate) fn split_pair(pair: &str) -> Result<(String, String), String> {
    pair.split_once('=')
        .map(|(field, value)| (field.to_owned(), value.to_owned()))
        .ok_or_else(|| format!("expected FIELD=VALUE, such as priority=high, not {pair:?}"))
}

pub(crate) fn set(field: &str, value: Value) -> Change {
    Change::SetField {
        field: field.to_owned(),
        label: String::new(),
        value,
        value_label: None,
    }
}

/// A `--set` value as the field's type stores it: a number or boolean where
/// the Scheme says so, else the text. Text that does not parse is passed on
/// for the store to refuse with the field's rules.
pub(crate) fn typed(scheme: &Scheme, field: &str, value: &str) -> Value {
    match scheme.field(field).map(|field| &field.field_type) {
        Some(FieldType::Number) => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or_else(|| json!(value), Value::Number),
        Some(FieldType::Boolean) => value
            .parse::<bool>()
            .map_or_else(|_| json!(value), Value::Bool),
        _ => json!(value),
    }
}

/// A label to add, or with `present` false, to remove.
pub(crate) fn label(elem: &str, present: bool) -> Change {
    let field = "labels".to_owned();
    let elem = elem.to_owned();
    if present {
        Change::Add {
            field,
            label: String::new(),
            elem,
            elem_label: None,
        }
    } else {
        Change::Remove {
            field,
            label: String::new(),
            elem,
            elem_label: None,
        }
    }
}

/// `value` as one POSIX shell word, for a fix to run as printed.
pub(crate) fn quoted(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.,:/=@+%".contains(&b));
    if plain {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use tm_core::scheme::Field;

    use super::*;

    #[test]
    fn a_set_value_takes_the_type_its_field_stores() {
        let mut scheme = Scheme::built_in();
        scheme.fields.push(Field {
            name: "points".into(),
            label: "Points".into(),
            field_type: FieldType::Number,
            builtin: false,
            derived: false,
            required: false,
            options: Vec::new(),
        });
        assert_eq!(typed(&scheme, "points", "3"), json!(3.0));
        assert_eq!(typed(&scheme, "points", "three"), json!("three"));
        assert_eq!(typed(&scheme, "archived", "true"), json!(true));
        assert_eq!(typed(&scheme, "title", "true"), json!("true"));
        assert_eq!(typed(&scheme, "nope", "1"), json!("1"));
    }
}
