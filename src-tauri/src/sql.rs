//! Splitting a script into individually executable statements.
//!
//! PostgreSQL parses a whole string as one simple query, but a GUI editor needs
//! one result per statement and needs to know which statement the cursor is
//! in. This is a small lexer that understands the constructs which can hide a
//! semicolon: single/double quotes, `E''` escapes, dollar quoting with tags,
//! nested block comments, and line comments. It is deliberately not a grammar.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitStatement {
    /// Position in the script, 0-based.
    pub index: usize,
    /// Statement text with surrounding whitespace removed and no trailing
    /// semicolon.
    pub sql: String,
    /// Byte offset of the first character within the original script.
    pub start: usize,
    /// Byte offset just past the last character.
    pub end: usize,
}

pub fn split(script: &str) -> Vec<SplitStatement> {
    let bytes = script.as_bytes();
    let len = bytes.len();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut has_code = false;

    while i < len {
        match bytes[i] {
            b'\'' => {
                has_code = true;
                i = skip_quoted(bytes, i, b'\'');
            }
            b'"' => {
                has_code = true;
                i = skip_quoted(bytes, i, b'"');
            }
            b'$' => {
                has_code = true;
                i = match dollar_tag_end(bytes, i) {
                    Some(tag_end) => skip_dollar_quoted(bytes, i, tag_end),
                    None => i + 1, // `$1` parameter or a lone dollar sign
                };
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = skip_block_comment(bytes, i);
            }
            b';' => {
                if let Some(mut stmt) = trim_statement(script, start, i, has_code) {
                    stmt.index = out.len();
                    out.push(stmt);
                }
                has_code = false;
                i += 1;
                start = i;
            }
            b if b.is_ascii_whitespace() => i += 1,
            _ => {
                has_code = true;
                i += utf8_len(bytes, i);
            }
        }
    }

    if let Some(mut stmt) = trim_statement(script, start, len, has_code) {
        stmt.index = out.len();
        out.push(stmt);
    }
    out
}

/// Builds a statement from `[start, end)` unless it is only whitespace and
/// comments.
fn trim_statement(
    script: &str,
    start: usize,
    end: usize,
    has_code: bool,
) -> Option<SplitStatement> {
    let raw = &script[start..end];
    let trimmed_start = start + (raw.len() - raw.trim_start().len());
    let trimmed_end = start + raw.trim_end().len();
    if trimmed_start >= trimmed_end {
        return None;
    }
    if !has_code && !contains_code(&script[trimmed_start..trimmed_end]) {
        return None;
    }
    Some(SplitStatement {
        index: 0,
        sql: script[trimmed_start..trimmed_end].to_string(),
        start: trimmed_start,
        end: trimmed_end,
    })
}

/// Whether the fragment holds anything executable (used only for fragments the
/// main loop did not already mark as code, i.e. comment-only tails).
fn contains_code(fragment: &str) -> bool {
    let bytes = fragment.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => i = skip_block_comment(bytes, i),
            b if b.is_ascii_whitespace() => i += 1,
            _ => return true,
        }
    }
    false
}

/// Skips a quoted run starting at `i` (which points at `quote`) and returns the
/// index just past the closing quote. Backslashes are treated as escapes only
/// for single quotes, covering `E'...'`; the cost of over-honouring is an
/// extended literal, which is the safe direction for a splitter.
fn skip_quoted(bytes: &[u8], mut i: usize, quote: u8) -> usize {
    i += 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if quote == b'\'' => i += 2,
            c if c == quote => {
                if bytes.get(i + 1) == Some(&quote) {
                    i += 2; // doubled quote inside the literal
                } else {
                    return i + 1;
                }
            }
            _ => i += utf8_len(bytes, i),
        }
    }
    bytes.len()
}

/// If `i` points at the `$` of a `$tag$` opener, returns the index just past
/// the opener.
fn dollar_tag_end(bytes: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < bytes.len() {
        let c = bytes[j];
        if c == b'$' {
            return Some(j + 1);
        }
        let valid = c == b'_' || c.is_ascii_alphanumeric();
        let valid = valid && !(j == i + 1 && c.is_ascii_digit());
        if !valid {
            return None;
        }
        j += 1;
    }
    None
}

fn skip_dollar_quoted(bytes: &[u8], open: usize, tag_end: usize) -> usize {
    let tag = &bytes[open..tag_end];
    let mut i = tag_end;
    while i < bytes.len() {
        if bytes[i] == b'$' && bytes[i..].starts_with(tag) {
            return i + tag.len();
        }
        i += 1;
    }
    bytes.len()
}

fn skip_block_comment(bytes: &[u8], mut i: usize) -> usize {
    let mut depth = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            depth += 1;
            i += 2;
        } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += utf8_len(bytes, i);
        }
    }
    bytes.len()
}

fn utf8_len(bytes: &[u8], i: usize) -> usize {
    match bytes[i] {
        b if b < 0x80 => 1,
        b if b < 0xE0 => 2,
        b if b < 0xF0 => 3,
        _ => 4,
    }
}

/// The statement the cursor is inside, or the statement ending just before the
/// cursor when the cursor sits between statements.
#[allow(dead_code)]
pub fn statement_at(script: &str, cursor: usize) -> Option<SplitStatement> {
    let statements = split(script);
    statements
        .iter()
        .find(|s| cursor >= s.start && cursor <= s.end + 1)
        .or_else(|| statements.iter().rev().find(|s| s.end <= cursor))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(script: &str) -> Vec<String> {
        split(script).into_iter().map(|s| s.sql).collect()
    }

    #[test]
    fn splits_simple_statements() {
        assert_eq!(
            texts("select 1; select 2;"),
            vec!["select 1".to_string(), "select 2".to_string()]
        );
    }

    #[test]
    fn keeps_semicolons_inside_literals() {
        assert_eq!(
            texts("select ';' as a; select 2"),
            vec!["select ';' as a".to_string(), "select 2".to_string()]
        );
        assert_eq!(
            texts("select \"we;ird\" from t"),
            vec!["select \"we;ird\" from t".to_string()]
        );
        assert_eq!(
            texts("select E'it\\'s; fine'; select 2"),
            vec!["select E'it\\'s; fine'".to_string(), "select 2".to_string()]
        );
        assert_eq!(
            texts("select 'it''s; fine'; select 2"),
            vec!["select 'it''s; fine'".to_string(), "select 2".to_string()]
        );
    }

    #[test]
    fn keeps_semicolons_inside_dollar_quotes() {
        assert_eq!(
            texts("create function f() returns int as $$\n begin; select 1; end; \n$$ language plpgsql; select 3"),
            vec![
                "create function f() returns int as $$\n begin; select 1; end; \n$$ language plpgsql"
                    .to_string(),
                "select 3".to_string()
            ]
        );
        assert_eq!(
            texts("select $tag$a;b$tag$; select 1"),
            vec!["select $tag$a;b$tag$".to_string(), "select 1".to_string()]
        );
    }

    #[test]
    fn ignores_comments() {
        assert_eq!(
            texts("-- a; b\nselect 1"),
            vec!["-- a; b\nselect 1".to_string()]
        );
        assert!(split("-- only a comment").is_empty());
        assert!(split("/* nested /* ; */ still */").is_empty());
        assert_eq!(
            texts("select 1 /* ; */; select 2"),
            vec!["select 1 /* ; */".to_string(), "select 2".to_string()]
        );
    }

    #[test]
    fn keeps_trailing_statement_without_semicolon() {
        assert_eq!(texts("select 1"), vec!["select 1".to_string()]);
        assert_eq!(texts("select 1;\n"), vec!["select 1".to_string()]);
    }

    #[test]
    fn offsets_point_at_source_text() {
        let script = "  select 1;\nselect 2";
        let stmts = split(script);
        assert_eq!(&script[stmts[0].start..stmts[0].end], "select 1");
        assert_eq!(&script[stmts[1].start..stmts[1].end], "select 2");
    }

    #[test]
    fn unicode_does_not_split_characters() {
        assert_eq!(
            texts("select '中文;字' as x; select '🙂'"),
            vec![
                "select '中文;字' as x".to_string(),
                "select '🙂'".to_string()
            ]
        );
    }

    #[test]
    fn finds_statement_at_cursor() {
        let script = "select 1;\nselect 2 ;\nselect 3";
        let cursor = script.find("2 ;").unwrap() + 1;
        assert_eq!(statement_at(script, cursor).unwrap().sql, "select 2");
    }
}
