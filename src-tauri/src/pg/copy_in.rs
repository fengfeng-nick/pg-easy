//! `COPY … FROM STDIN`，SQL 转储就是靠它携带数据行。
//!
//! 语句和它的数据作为一次简单查询交互发出：服务器用 `CopyInResponse` 回应
//! `COPY`，随后数据以 `CopyData` 消息跟进，`CopyDone` 结束。本类型把这三步
//! 从脚本里抽出来，好让连接按顺序执行——而且只有在服务器索要之后才发送数据。
//! 为一条被服务器拒绝的语句发送数据，会被当成下一条语句的开头。

use std::io::Write;

/// COPY 数据以一行只有反斜杠和点的内容结束。这个标记是 psql 的，不是服务器
/// 的：它会被从数据里切掉，数据的结束改用 `CopyDone` 告知。
const END_OF_DATA: &str = "\\.";

/// 一条 `COPY … FROM STDIN` 语句，连同其后跟随的数据。
#[derive(Debug)]
pub struct CopyInScript {
    /// 语句本身，不含末尾的分号：执行器会在数据之前写上终止符，因为数据不在
    /// 查询文本里。
    pub sql: String,
    /// zlib 压缩后的 copy 格式文本，`COPY … FROM STDIN` 默认按文本读取。
    /// 数据行是转储的主体，压缩率很高。
    pub data: Vec<u8>,
}

impl CopyInScript {
    /// 为 `statement` 和随后跟随的 `data` 构建脚本。`data` 与文件里完全一致
    /// ——含换行，但不含 `\.` 标记。语句不是 `COPY … FROM STDIN` 时返回
    /// `None`。
    pub fn split(statement: &str, data: &str) -> Option<Self> {
        if !reads_from_stdin(statement) {
            return None;
        }
        Some(Self {
            sql: statement.trim().trim_end_matches(';').trim_end().to_string(),
            data: compress(data.as_bytes()).ok()?,
        })
    }
}

/// 语句的可执行文本：去掉开头的空白和注释，这样转储的 `--` 头注释块不会
/// 遮住它所引导的那条语句。
pub(crate) fn code_start(statement: &str) -> &str {
    let bytes = statement.as_bytes();
    let mut i = 0usize;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes[i..].starts_with(b"--") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            let mut depth = 1usize;
            i += 2;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        return &statement[i..];
    }
}

/// `statement` 是否为一条从客户端读取行的 `COPY`。
///
/// 只看数据源：`COPY … TO STDOUT`、`COPY … FROM 'file'` 和
/// `COPY … FROM PROGRAM '…'` 都不需要客户端发送数据。列清单可能位于目标和
/// `FROM` 之间，还可能跨行，所以匹配做在语句尾部而不是第一行上。
pub(crate) fn reads_from_stdin(statement: &str) -> bool {
    let code = code_start(statement);
    let compact = code.split_ascii_whitespace().collect::<Vec<_>>().join(" ");
    let lower = compact.to_ascii_lowercase();
    if !lower.starts_with("copy ") {
        return false;
    }
    // 最后一个 `FROM` 才是本语句的：更早的那些属于
    // `COPY (SELECT … FROM …) TO …` 里的子查询。
    let Some(index) = lower.rfind(" from ") else {
        return false;
    };
    lower[index + 6..].trim_start().starts_with("stdin")
}

/// 从 `text[0]` 开始的 copy 数据在哪里结束：标记行的偏移，以及它换行之后
/// 一位的偏移。没有标记行结束该块时返回 `None`，说明文件被截断了。
pub(crate) fn find_end_of_data(text: &str) -> Option<(usize, usize)> {
    let mut offset = 0usize;
    while offset < text.len() {
        let line_end = text[offset..]
            .find('\n')
            .map(|index| offset + index)
            .unwrap_or(text.len());
        let line = text[offset..line_end].trim_end_matches('\r');
        if line == END_OF_DATA {
            let after = if line_end < text.len() {
                line_end + 1
            } else {
                line_end
            };
            return Some((offset, after));
        }
        if line_end >= text.len() {
            return None;
        }
        offset = line_end + 1;
    }
    None
}

/// 把 `data` 压缩后发上线。服务器会解压，所以语句看到的内容不变。
pub fn compress(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(data)?;
    encoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inflate(data: &[u8]) -> String {
        use std::io::Read;
        let mut out = String::new();
        flate2::read::ZlibDecoder::new(data)
            .read_to_string(&mut out)
            .expect("inflate");
        out
    }

    fn split(statement: &str, data: &str) -> Option<CopyInScript> {
        CopyInScript::split(statement, data)
    }

    #[test]
    fn splits_a_copy_block() {
        let script = split("copy public.t (a) from stdin;", "1\tx\n2\t\\N\n").expect("a copy block");
        assert_eq!(script.sql, "copy public.t (a) from stdin");
        assert_eq!(inflate(&script.data), "1\tx\n2\t\\N\n");
    }

    #[test]
    fn keeps_quotes_and_escapes_in_the_data() {
        let data = "he said \\'hi\\'\nline\\ntwo\n";
        let script = split("COPY t (a) FROM stdin;", data).expect("block");
        assert_eq!(inflate(&script.data), data);
    }

    #[test]
    fn accepts_an_empty_table() {
        let script = split("copy t from stdin;", "").expect("block");
        assert_eq!(inflate(&script.data), "");
    }

    #[test]
    fn ignores_other_statements() {
        assert!(split("select 1", "").is_none());
        assert!(split("create table t (a text)", "").is_none());
        assert!(split("copy t to stdout", "").is_none());
        assert!(split("copy t from '/tmp/t.csv'", "").is_none());
        assert!(split("copy (select * from t) to stdout", "").is_none());
        assert!(split("copy t from program 'cat /tmp/t'", "").is_none());
    }

    #[test]
    fn reads_past_a_dump_header() {
        let statement = "--\n-- Data for Name: t; Type: TABLE DATA\n--\n\nCOPY public.t (a) FROM stdin;";
        assert!(reads_from_stdin(statement));
        let script = split(statement, "1\n").expect("block");
        assert_eq!(script.sql, statement.trim().trim_end_matches(';').trim_end());
    }

    #[test]
    fn accepts_names_with_parens_and_column_lists_over_lines() {
        assert!(reads_from_stdin("copy \"t (x)\" (a) from stdin;"));
        assert!(reads_from_stdin("copy t (\n  a,\n  b\n) from stdin"));
    }

    #[test]
    fn ends_data_at_the_marker() {
        let text = "1\n2\n\\.\nselect 1;\n";
        let (start, after) = find_end_of_data(text).expect("a marker");
        assert_eq!(&text[..start], "1\n2\n");
        assert_eq!(&text[after..], "select 1;\n");
    }

    #[test]
    fn ends_data_at_a_marker_with_carriage_returns() {
        let text = "1\r\n\\.\r\n";
        let (start, after) = find_end_of_data(text).expect("a marker");
        assert_eq!(&text[..start], "1\r\n");
        assert_eq!(&text[after..], "");
    }

    #[test]
    fn refuses_data_that_is_not_terminated() {
        assert!(find_end_of_data("1\n2\n").is_none());
        assert!(find_end_of_data("").is_none());
    }
}
