//! 把一个脚本切分成可单独执行的语句。
//!
//! PostgreSQL 会把整个字符串当作一条简单查询解析，但图形化编辑器需要每条
//! 语句一个结果，还需要知道光标在哪条语句里。这是一个小词法器，理解那些能
//! 藏住分号的构造：单/双引号、`E''` 转义、带 tag 的美元引用、嵌套块注释和
//! 行注释。它刻意不构成完整的语法分析器。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitStatement {
    /// 在脚本中的位置，从 0 开始。
    pub index: usize,
    /// 语句文本，去掉了首尾空白，且不含末尾分号。
    pub sql: String,
    /// 首字符在原脚本中的字节偏移。
    pub start: usize,
    /// 最后一个字符之后的字节偏移。
    pub end: usize,
}

pub fn split(script: &str) -> Vec<SplitStatement> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(found) = scan(script, offset) {
        out.push(SplitStatement {
            index: out.len(),
            sql: script[found.start..found.end].to_string(),
            start: found.start,
            end: found.end,
        });
        offset = found.next;
    }
    out
}

/// `from` 之后的下一条语句，`index` 始终为 0，由调用方编号。
///
/// 与 [`split`] 不同，这里只扫描到第一条语句结束为止：导入时每跳过一个
/// `COPY` 数据块都要从干净的边界重新开始，而整份重扫会让大文件变成平方级。
pub fn next_statement(script: &str, from: usize) -> Option<SplitStatement> {
    let found = scan(script, from)?;
    Some(SplitStatement {
        index: 0,
        sql: script[found.start..found.end].to_string(),
        start: found.start,
        end: found.end,
    })
}

/// 一条语句在脚本中的位置，以及它结束后可以继续扫描的偏移。
struct Found {
    start: usize,
    end: usize,
    /// 终止分号之后（或文本末尾）。
    next: usize,
}

fn scan(script: &str, from: usize) -> Option<Found> {
    let bytes = script.as_bytes();
    let len = bytes.len();
    let mut start = from.min(len);
    let mut i = start;
    let mut has_code = false;
    if start >= len {
        return None;
    }

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
                    None => i + 1, // `$1` 参数或一个单独的美元符号
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
                if let Some(statement) = trim_statement(script, start, i, has_code) {
                    return Some(Found {
                        start: statement.start,
                        end: statement.end,
                        next: i + 1,
                    });
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

    trim_statement(script, start, len, has_code).map(|statement| Found {
        start: statement.start,
        end: statement.end,
        next: len,
    })
}

/// 由 `[start, end)` 构造一条语句，除非这段只有空白和注释。
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

/// 片段里是否有可执行内容（只用于主循环未标记为代码的片段，也就是只剩注释
/// 的尾部）。
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

/// 跳过从 `i` 开始的一段引号串（`i` 指向 `quote`），返回闭合引号之后一位的
/// 索引。反斜杠只对单引号按转义处理，覆盖 `E'...'`；多认成转义的代价是把
/// 普通串当成扩展字面量，这对拆分器来说是安全的方向。
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

/// 若 `i` 指向 `$tag$` 开头的 `$`，返回开头之后一位的索引。
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

/// 光标所在的语句；光标正好在两条语句之间时，返回它之前刚结束的那条。
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
