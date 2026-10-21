//! Field changes as the writing commands build them from their flags.

use std::collections::BTreeSet;
use std::io::{self, IsTerminal as _, Read as _};

use serde_json::{Value, json};
use tm_core::Change;
use tm_core::scheme::{FieldType, Scheme};

use crate::error::CliError;

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

/// Refuses a field named twice, since the later value would silently win.
pub(crate) fn given_once<'a>(fields: impl IntoIterator<Item = &'a String>) -> Result<(), CliError> {
    let mut seen = BTreeSet::new();
    for field in fields {
        if !seen.insert(field) {
            return Err(
                CliError::new("invalid_value", format!("{field} is given more than once"))
                    .with("field", field.as_str()),
            );
        }
    }
    Ok(())
}

/// `--set` values with a `-` read from stdin. Stdin is read once, never from
/// a terminal, and a blank value is refused with `blank`'s advice.
pub(crate) fn read_stdin(
    set: &[(String, String)],
    blank: &str,
) -> Result<Vec<(String, String)>, CliError> {
    let from_stdin: Vec<&str> = set
        .iter()
        .filter(|(_, value)| value == "-")
        .map(|(field, _)| field.as_str())
        .collect();
    if from_stdin.len() > 1 {
        return Err(CliError::new(
            "invalid_value",
            format!(
                "only one field can read stdin, not {}",
                from_stdin.join(", ")
            ),
        ));
    }
    let read = match from_stdin.first() {
        Some(field) => Some(stdin(field, blank)?),
        None => None,
    };
    Ok(set
        .iter()
        .map(|(field, value)| match (value.as_str(), &read) {
            ("-", Some(read)) => (field.clone(), read.clone()),
            _ => (field.clone(), value.clone()),
        })
        .collect())
}

fn stdin(field: &str, blank: &str) -> Result<String, CliError> {
    let read = read_stdin_text(field)?;
    let read = read.trim_end_matches(['\n', '\r']).to_owned();
    if read.trim().is_empty() {
        return Err(refused(field, &format!("is blank; {blank}")));
    }
    Ok(read)
}

/// All of stdin as `field`'s text, refused from a terminal, where reading
/// would wait on a prompt nobody sees.
pub(crate) fn read_stdin_text(field: &str) -> Result<String, CliError> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        return Err(refused(field, "is a terminal; pipe the value in"));
    }
    let mut read = String::new();
    match stdin.read_to_string(&mut read) {
        Ok(_) => Ok(read),
        Err(err) if err.kind() == io::ErrorKind::InvalidData => Err(refused(field, "is not UTF-8")),
        Err(err) => Err(CliError::new("io", format!("stdin: {err}")).with("path", "-")),
    }
}

fn refused(field: &str, reason: &str) -> CliError {
    CliError::new("invalid_value", format!("{field} from stdin {reason}"))
        .with("field", field)
        .with("reason", reason)
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
