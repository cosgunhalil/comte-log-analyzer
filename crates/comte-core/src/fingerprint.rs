//! Stable fingerprints used to collapse duplicate entries ("×42") and to match
//! entries across two logs when comparing them.
//!
//! The hash is FNV-1a (64-bit): tiny, dependency-free and stable across Rust
//! versions and runs, unlike `std`'s `DefaultHasher`.

use crate::model::LogEntry;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
const FIELD_SEPARATOR: u8 = 0xff; // never appears in UTF-8 text

#[derive(Debug, Clone, Copy)]
struct Fnv1a(u64);

impl Fnv1a {
    fn new() -> Self {
        Self(FNV_OFFSET)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }

    fn field(&mut self, bytes: &[u8]) {
        self.write(bytes);
        self.write(&[FIELD_SEPARATOR]);
    }
}

/// Fingerprint of an entry: category + normalized message + culprit location.
///
/// Two entries that differ only in numbers, ids or timings (e.g. "took 12ms"
/// vs "took 15ms") get the same fingerprint.
pub fn of_entry(entry: &LogEntry) -> u64 {
    let mut hasher = Fnv1a::new();
    hasher.field(entry.category.label().as_bytes());
    hasher.field(normalize_message(&entry.message).as_bytes());
    if let Some(culprit) = entry.culprit() {
        hasher.field(culprit.path.as_bytes());
        hasher.field(&culprit.line.unwrap_or(0).to_le_bytes());
    }
    hasher.0
}

/// Like [`of_entry`] but ignoring the culprit's line number. Used to match
/// entries across two logs: editing a script between runs moves an error to
/// another line, and it should still count as the same problem.
pub fn across_runs(entry: &LogEntry) -> u64 {
    let mut hasher = Fnv1a::new();
    hasher.field(entry.category.label().as_bytes());
    hasher.field(normalize_message(&entry.message).as_bytes());
    if let Some(culprit) = entry.culprit() {
        hasher.field(culprit.path.as_bytes());
    }
    hasher.0
}

/// Replaces volatile tokens with `#` and collapses whitespace:
///
/// - plain numbers: `42` → `#`
/// - numbers with a unit suffix: `304ms` → `#ms`
/// - hex literals: `0x00046` → `#`
/// - long hex ids / GUIDs (8+ hex chars with a digit): `061c453a…` → `#`
///
/// Identifiers such as `CS0103` or `Player2D` are kept, so different compiler
/// errors stay different.
pub fn normalize_message(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut token = String::new();
    let mut pending_space = false;

    let flush = |token: &mut String, out: &mut String, pending_space: &mut bool| {
        if token.is_empty() {
            return;
        }
        if *pending_space && !out.is_empty() {
            out.push(' ');
        }
        *pending_space = false;
        normalize_token(token, out);
        token.clear();
    };

    for c in message.chars() {
        if c.is_alphanumeric() || c == '_' {
            token.push(c);
            continue;
        }
        flush(&mut token, &mut out, &mut pending_space);
        if c.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        }
    }
    flush(&mut token, &mut out, &mut pending_space);
    out
}

fn normalize_token(token: &str, out: &mut String) {
    let bytes = token.as_bytes();

    let is_hex_literal = bytes.len() > 2
        && (token.starts_with("0x") || token.starts_with("0X"))
        && bytes[2..].iter().all(u8::is_ascii_hexdigit);
    let is_long_hex_id = bytes.len() >= 8
        && bytes.iter().all(u8::is_ascii_hexdigit)
        && bytes.iter().any(u8::is_ascii_digit);
    if is_hex_literal || is_long_hex_id {
        out.push('#');
        return;
    }

    let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits > 0 && bytes[digits..].iter().all(u8::is_ascii_alphabetic) {
        out.push('#');
        out.push_str(&token[digits..]); // unit suffix such as "ms", "MB"
        return;
    }

    out.push_str(token);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Category, Severity, SourceRef};

    #[test]
    fn normalizes_volatile_tokens() {
        assert_eq!(
            normalize_message("Domain Reload Profiling: 1304ms"),
            "Domain Reload Profiling: #ms"
        );
        assert_eq!(
            normalize_message(
                "Asset Pipeline Refresh (id=061c453a100b521459d8cb43e1812596): Total: 3.530 seconds"
            ),
            "Asset Pipeline Refresh (id=#): Total: #.# seconds"
        );
        assert_eq!(
            normalize_message("at Foo.Bar () [0x00046] in <19bd4cc6a4004258bf538b05ac1133d6>:0"),
            "at Foo.Bar () [#] in <#>:#"
        );
    }

    #[test]
    fn keeps_identifiers_and_collapses_whitespace() {
        assert_eq!(
            normalize_message("  error   CS0103:\tThe name 'Player2D' "),
            "error CS0103: The name 'Player2D'"
        );
    }

    #[test]
    fn same_message_with_different_numbers_shares_a_fingerprint() {
        let a = LogEntry::new(
            Severity::Info,
            Category::UserLog,
            "Loaded 12 items in 40ms",
            1..2,
        );
        let b = LogEntry::new(
            Severity::Info,
            Category::UserLog,
            "Loaded 99 items in 7ms",
            5..6,
        );
        assert_eq!(of_entry(&a), of_entry(&b));
    }

    #[test]
    fn location_and_category_distinguish_fingerprints() {
        let base = LogEntry::new(Severity::Error, Category::UserLog, "boom", 1..2);
        let here = base
            .clone()
            .with_source(SourceRef::new("Assets/A.cs").with_line(1));
        let there = base
            .clone()
            .with_source(SourceRef::new("Assets/A.cs").with_line(2));
        let mut other_category = here.clone();
        other_category.category = Category::Exception;

        assert_ne!(of_entry(&here), of_entry(&there));
        assert_ne!(of_entry(&here), of_entry(&other_category));
        assert_ne!(of_entry(&base), of_entry(&here));
    }

    #[test]
    fn fingerprint_is_stable_across_runs() {
        // If this changes, saved comparisons and tests relying on it break.
        let entry = LogEntry::new(Severity::Info, Category::Other, "hello", 1..2);
        assert_eq!(of_entry(&entry), of_entry(&entry.clone()));
        let mut h = Fnv1a::new();
        h.write(b"a");
        assert_eq!(
            h.0, 0xaf63_dc4c_8601_ec8c,
            "FNV-1a reference value for \"a\""
        );
    }
}
