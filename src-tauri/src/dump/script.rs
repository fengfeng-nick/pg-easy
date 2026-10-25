//! 转储脚本的外壳：文件头、失败可重试的 do 块。

use super::util::unix_now;

pub(crate) fn is_key(kind: &str) -> bool {
    matches!(kind, "p" | "u" | "x")
}

/// 一串键约束，作为 `do` 块的步骤：每条语句运行在 `wrapped` 打开的块内，
/// 收尾的 `end` 由调用方负责。
///
/// 语句执行时，服务器可能已经不认识前面某条语句创建的对象（给表加上主键后
/// 再从目录里重读这张表，就是实际出现过的情况）；这取决于对象上次解析之后
/// 脚本增长了多少，无法预判。失败的语句会再试一次，试之前先让服务器重新读
/// 一遍目录。
pub(crate) fn push_steps(out: &mut String, statements: &[String]) {
    for (index, statement) in statements.iter().enumerate() {
        let retry = if index + 1 == statements.len() {
            "raise"
        } else {
            "select 1 from pg_catalog.pg_class limit 0"
        };
        let close = if statement.starts_with("do $") { " end" } else { "" };
        out.push_str(&format!(
            "begin\n    {statement}\n    exception when others then\n    {retry}{close};\nend;\n"
        ));
    }
}

/// 把 `body`（一串以 `;` 结尾的语句）放进自己的 `do` 块里，失败时先让服务器
/// 重读目录再重试一次（原因见 `run_steps`）。该块在单个事务中运行：一旦失败
/// 整段回滚，psql 随后停在重新抛出的错误上，效果与停在原语句上相同。
/// `body` 必须以自身的 `;` 结尾语句开头，或者为空。
pub(crate) fn wrapped(header: String, body: &str) -> String {
    if !body.contains(';') {
        return header + body;
    }
    format!("{header}do $steps$\nbegin\n{body}exception when others then\nraise;\nend $steps$;\n\n")
}

/// 脚本开头：让回放能精确复现数据行的设置，以及转储发生的时刻。
pub(crate) fn header(title: &str) -> String {
    format!(
        "--\n-- {title} — exported by pg easy at {} (unix time)\n--\n\n\
         SET statement_timeout = 0;\n\
         SET lock_timeout = 0;\n\
         SET idle_in_transaction_session_timeout = 0;\n\
         SET client_encoding = 'UTF8';\n\
         SET standard_conforming_strings = on;\n\
         SET check_function_bodies = false;\n\
         SET client_min_messages = warning;\n\
         SET row_security = off;\n\n",
        unix_now()
    )
}
