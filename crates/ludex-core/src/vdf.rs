//! Minimal parser for the top level of a Valve VDF document.
//!
//! VDF (Valve Data Format) is the quoted-pair serialisation Steam uses
//! for `appmanifest_*.acf`, `loginusers.vdf`, and similar files. The
//! full grammar has nesting, macros, and conditional blocks; ludex only
//! needs the flat case — one layer of `"key" "value"` pairs inside the
//! top object — so this module deliberately does *not* track nesting
//! depth. Nested keys with the same name as a top-level key will
//! match; every VDF document we care about keeps the fields we read
//! (`name`, `StateFlags`) at the top.
//!
//! The parsers are total: every input shape returns `None` rather than
//! panicking, and the property tests in both callers (daemon's Steam
//! source, enrich crate's Steam source) have a "never panics" case
//! driving them.

/// Iterator over every value on a `"key" "value"` line, at any nesting
/// depth (this parser does not track depth — see the module docs).
fn values_for_key<'a>(content: &'a str, key: &str) -> impl Iterator<Item = String> + 'a {
    let needle = format!("\"{key}\"");
    content.lines().filter_map(move |line| {
        let rest = line.trim_start().strip_prefix(&needle)?;
        let after_open = rest.trim_start().strip_prefix('"')?;
        let end = after_open.find('"')?;
        Some(after_open[..end].to_owned())
    })
}

/// Extract the first value associated with a `"key" "value"` line at
/// the top level of a VDF document.
///
/// Returns `None` if the key is not present, the value is not a
/// quoted string, or the opening quote is never closed.
#[must_use]
pub fn parse_top_level_string(content: &str, key: &str) -> Option<String> {
    values_for_key(content, key).next()
}

/// Like [`parse_top_level_string`] but parses the value as `u64`.
/// Returns `None` when the key is missing, the value is malformed, or
/// the integer does not fit.
#[must_use]
pub fn parse_top_level_u64(content: &str, key: &str) -> Option<u64> {
    parse_top_level_string(content, key).and_then(|s| s.parse().ok())
}

/// Every value associated with a `"key" "value"` line, in document
/// order, regardless of nesting depth. Used for keys that legitimately
/// repeat — `libraryfolders.vdf` has one `"path"` per Steam library.
#[must_use]
pub fn parse_all_values(content: &str, key: &str) -> Vec<String> {
    values_for_key(content, key).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const ACF: &str = "\
\"AppState\"
{
\t\"appid\"\t\t\"228980\"
\t\"name\"\t\t\"Steamworks Common Redistributables\"
\t\"StateFlags\"\t\t\"4\"
}";

    #[test]
    fn parses_string_field() {
        assert_eq!(
            parse_top_level_string(ACF, "name").as_deref(),
            Some("Steamworks Common Redistributables")
        );
    }

    #[test]
    fn parses_u64_field() {
        assert_eq!(parse_top_level_u64(ACF, "StateFlags"), Some(4));
    }

    #[test]
    fn missing_key_returns_none() {
        assert_eq!(parse_top_level_string(ACF, "publisher"), None);
        assert_eq!(parse_top_level_u64(ACF, "build"), None);
    }

    #[test]
    fn unterminated_value_returns_none() {
        let content = "\"name\" \"Unterminated\n";
        assert_eq!(parse_top_level_string(content, "name"), None);
    }

    const LIBRARYFOLDERS: &str = "\
\"libraryfolders\"
{
\t\"0\"
\t{
\t\t\"path\"\t\t\"/home/u/.local/share/Steam\"
\t\t\"label\"\t\t\"\"
\t}
\t\"1\"
\t{
\t\t\"path\"\t\t\"/mnt/games/SteamLibrary\"
\t\t\"label\"\t\t\"\"
\t}
}";

    #[test]
    fn parse_all_values_collects_every_library_path() {
        assert_eq!(
            parse_all_values(LIBRARYFOLDERS, "path"),
            vec![
                "/home/u/.local/share/Steam".to_owned(),
                "/mnt/games/SteamLibrary".to_owned(),
            ]
        );
    }

    #[test]
    fn parse_all_values_missing_key_is_empty() {
        assert!(parse_all_values(LIBRARYFOLDERS, "contentid").is_empty());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn string_parser_never_panics(s in "\\PC{0,500}", key in "[a-zA-Z_]{1,20}") {
            let _ = parse_top_level_string(&s, &key);
        }

        #[test]
        fn u64_parser_never_panics(s in "\\PC{0,500}", key in "[a-zA-Z_]{1,20}") {
            let _ = parse_top_level_u64(&s, &key);
        }

        #[test]
        fn all_values_parser_never_panics(s in "\\PC{0,500}", key in "[a-zA-Z_]{1,20}") {
            let _ = parse_all_values(&s, &key);
        }
    }
}
