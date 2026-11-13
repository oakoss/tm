//! A person's name, a record of its own keyed by their email (ADR 0019).

use sha2::{Digest, Sha256};
use uuid::{Builder, Uuid};

/// The longest name tm records, in characters.
pub const MAX_NAME_CHARS: usize = 200;

/// The record holding the name of the person writing as `email`: one id per
/// email, ignoring case and surrounding spaces, so every Replica writes to
/// the same record.
#[must_use]
pub fn id(email: &str) -> Uuid {
    let digest = Sha256::new()
        .chain_update(b"tm person\0")
        .chain_update(key(email).as_bytes())
        .finalize();
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    Builder::from_custom_bytes(bytes).into_uuid()
}

/// Line separators and bidirectional controls, which break a shown name's
/// line or reorder the text around it. Joiners and variation selectors
/// stay, as emoji need them.
#[must_use]
pub const fn breaks(c: char) -> bool {
    matches!(c,
        '\u{061C}'
        | '\u{200E}'..='\u{200F}'
        | '\u{2028}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}')
}

/// `email` as people are told apart: trimmed and lowercase.
#[must_use]
pub fn key(email: &str) -> String {
    email.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_record_per_email_whatever_its_case() {
        assert_eq!(id(" Jace@Example.com"), id("jace@example.com"));
        assert_ne!(id("jace@example.com"), id("kim@example.com"));
        assert_eq!(id("jace@example.com").get_version_num(), 8);
    }
}
