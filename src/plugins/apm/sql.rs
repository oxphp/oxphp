//! SQL obfuscation for APM datastore spans.
//!
//! When auto-instrumentation hooks intercept `PDO::query` or `mysqli::query`,
//! the raw SQL text must be obfuscated before storing as a `db.statement` span
//! attribute. This prevents PII (emails, passwords, tokens) from leaking into
//! tracing backends.
//!
//! The obfuscator runs a single-pass byte scan over the input — no regex, no
//! allocations beyond the output `String`.

/// Replace literal values in a SQL string with `?` placeholders.
///
/// Handles:
/// - Single-quoted strings: `'hello'` → `?` (with `''` and `\'` escapes)
/// - Double-quoted strings: `"hello"` → `?` (with escaped quotes)
/// - Numeric literals: `42`, `3.14`, `-7` → `?`
/// - Negative numbers when preceded by a valid prefix character
///
/// Preserves identifiers, keywords, operators, comments, and existing `?`
/// placeholders.
pub fn obfuscate(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len);
    let mut i = 0;

    while i < len {
        let b = bytes[i];

        // Single-quoted string literal.
        if b == b'\'' {
            out.push('?');
            i += 1;
            while i < len {
                let c = bytes[i];
                if c == b'\\' {
                    // Backslash escape — skip next char.
                    i += 2;
                    continue;
                }
                if c == b'\'' {
                    i += 1;
                    // Doubled quote escape — keep going.
                    if i < len && bytes[i] == b'\'' {
                        i += 1;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            continue;
        }

        // Double-quoted string literal.
        if b == b'"' {
            out.push('?');
            i += 1;
            while i < len {
                let c = bytes[i];
                if c == b'\\' {
                    i += 2;
                    continue;
                }
                if c == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // Negative number: only when preceded by a valid prefix.
        if b == b'-' && i + 1 < len && bytes[i + 1].is_ascii_digit() {
            let is_neg_prefix = if i == 0 {
                true
            } else {
                matches!(
                    bytes[i - 1],
                    b' ' | b'\t' | b'\n' | b'\r' | b'(' | b',' | b'=' | b'<' | b'>' | b'!'
                )
            };
            if is_neg_prefix {
                out.push('?');
                i += 1; // skip '-'
                        // Skip digits.
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                // Skip decimal part.
                if i < len && bytes[i] == b'.' && i + 1 < len && bytes[i + 1].is_ascii_digit() {
                    i += 1; // skip '.'
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                continue;
            }
            // Not a negative number prefix — fall through to emit '-'.
            out.push(b as char);
            i += 1;
            continue;
        }

        // Numeric literal (not part of an identifier).
        if b.is_ascii_digit() {
            // Check if this digit is part of an identifier (preceded by a letter or underscore).
            let part_of_ident = if i > 0 {
                let prev = bytes[i - 1];
                prev.is_ascii_alphanumeric() || prev == b'_'
            } else {
                false
            };

            if part_of_ident {
                // Part of an identifier like `col1` — emit as-is.
                out.push(b as char);
                i += 1;
                continue;
            }

            out.push('?');
            // Skip all digits.
            while i < len && bytes[i].is_ascii_digit() {
                i += 1;
            }
            // Skip decimal part.
            if i < len && bytes[i] == b'.' && i + 1 < len && bytes[i + 1].is_ascii_digit() {
                i += 1; // skip '.'
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            continue;
        }

        // Everything else: pass through.
        out.push(b as char);
        i += 1;
    }

    out
}

/// Extract the SQL operation from the first keyword.
///
/// Returns one of: `"SELECT"`, `"INSERT"`, `"UPDATE"`, `"DELETE"`, `"CREATE"`,
/// `"ALTER"`, `"DROP"`, `"TRUNCATE"`, or `"OTHER"` for anything else.
///
/// Case-insensitive; leading whitespace is trimmed.
pub fn extract_operation(sql: &str) -> &'static str {
    let trimmed = sql.trim_start();
    // Find the end of the first word.
    let end = trimmed
        .find(|c: char| c.is_ascii_whitespace() || c == '(')
        .unwrap_or(trimmed.len());
    let word = &trimmed[..end];

    if word.eq_ignore_ascii_case("SELECT") {
        "SELECT"
    } else if word.eq_ignore_ascii_case("INSERT") {
        "INSERT"
    } else if word.eq_ignore_ascii_case("UPDATE") {
        "UPDATE"
    } else if word.eq_ignore_ascii_case("DELETE") {
        "DELETE"
    } else if word.eq_ignore_ascii_case("CREATE") {
        "CREATE"
    } else if word.eq_ignore_ascii_case("ALTER") {
        "ALTER"
    } else if word.eq_ignore_ascii_case("DROP") {
        "DROP"
    } else if word.eq_ignore_ascii_case("TRUNCATE") {
        "TRUNCATE"
    } else {
        "OTHER"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_obfuscate_single_quoted_string() {
        assert_eq!(
            obfuscate("WHERE email = 'john@example.com'"),
            "WHERE email = ?"
        );
    }

    #[test]
    fn test_obfuscate_numeric_literal() {
        assert_eq!(
            obfuscate("WHERE id = 42 AND age > 25"),
            "WHERE id = ? AND age > ?"
        );
    }

    #[test]
    fn test_obfuscate_float() {
        assert_eq!(obfuscate("price > 19.99"), "price > ?");
    }

    #[test]
    fn test_obfuscate_negative_number() {
        assert_eq!(obfuscate("val = -42"), "val = ?");
    }

    #[test]
    fn test_obfuscate_escaped_quote() {
        assert_eq!(obfuscate("name = 'it''s'"), "name = ?");
    }

    #[test]
    fn test_obfuscate_backslash_escape() {
        assert_eq!(obfuscate("name = 'it\\'s'"), "name = ?");
    }

    #[test]
    fn test_obfuscate_multiple_params() {
        assert_eq!(obfuscate("VALUES ('John', 30)"), "VALUES (?, ?)");
    }

    #[test]
    fn test_obfuscate_already_parameterized() {
        assert_eq!(
            obfuscate("WHERE id = ? AND name = ?"),
            "WHERE id = ? AND name = ?"
        );
    }

    #[test]
    fn test_obfuscate_preserves_identifiers_with_numbers() {
        assert_eq!(
            obfuscate("SELECT col1, col2 FROM table1"),
            "SELECT col1, col2 FROM table1"
        );
    }

    #[test]
    fn test_obfuscate_empty_string() {
        assert_eq!(obfuscate(""), "");
    }

    #[test]
    fn test_obfuscate_in_clause() {
        assert_eq!(obfuscate("IN (1, 2, 3)"), "IN (?, ?, ?)");
    }

    #[test]
    fn test_extract_operation() {
        assert_eq!(extract_operation("SELECT * FROM users"), "SELECT");
        assert_eq!(extract_operation("select * from users"), "SELECT");
        assert_eq!(extract_operation("INSERT INTO users VALUES (1)"), "INSERT");
        assert_eq!(extract_operation("UPDATE users SET name = 'x'"), "UPDATE");
        assert_eq!(extract_operation("DELETE FROM users"), "DELETE");
        assert_eq!(extract_operation("CREATE TABLE users (id INT)"), "CREATE");
        assert_eq!(extract_operation("EXPLAIN SELECT 1"), "OTHER");
    }
}
