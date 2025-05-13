/// Escapes a TOML key if it contains characters that require escaping.
/// According to TOML spec, keys with dots, spaces, or other special characters
/// should be quoted.
pub fn escape_toml_key(key: &str) -> String {
    // Characters that require escaping in TOML keys
    if key.contains([
        '.', ' ', '\t', '\n', '#', '=', '\'', '"', '[', ']', '{', '}',
    ]) {
        format!("\"{}\"", key.replace('\"', "\\\""))
    } else {
        key.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_toml_key() {
        // Keys without special characters don't get modified
        assert_eq!(escape_toml_key("simple"), "simple");
        assert_eq!(escape_toml_key("simple_key"), "simple_key");
        assert_eq!(escape_toml_key("simpleKey"), "simpleKey");

        // Keys with special characters get properly quoted
        assert_eq!(escape_toml_key("test.test"), "\"test.test\"");
        assert_eq!(escape_toml_key("key with space"), "\"key with space\"");
        assert_eq!(escape_toml_key("key.with.dots"), "\"key.with.dots\"");

        // Keys with quotes get properly escaped
        assert_eq!(
            escape_toml_key("key\"with\"quotes"),
            "\"key\\\"with\\\"quotes\""
        );

        // Keys with other special characters
        assert_eq!(escape_toml_key("key#with#hash"), "\"key#with#hash\"");
        assert_eq!(escape_toml_key("key=with=equals"), "\"key=with=equals\"");
        assert_eq!(
            escape_toml_key("key[with]brackets"),
            "\"key[with]brackets\""
        );
    }
}
