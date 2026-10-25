//! 把服务器的 ErrorResponse / NoticeResponse 收成应用错误。

use fallible_iterator::FallibleIterator;
use postgres_protocol::message::backend;

use crate::error::AppError;

/// `ErrorResponse` 里值得显示的部分。
#[derive(Debug, Clone)]
pub struct DbError {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
    pub position: Option<usize>,
    /// 失败语句的位置，当错误是在挂到某条语句的函数（它的触发器、规则或
    /// 约束）里抛出时由服务器设置。从 1 开始。
    pub internal_position: Option<usize>,
}

impl DbError {
    pub(crate) fn parse(body: &backend::ErrorResponseBody) -> std::io::Result<Self> {
        Self::parse_fields(body.fields())
    }

    pub(crate) fn parse_notice(body: &backend::NoticeResponseBody) -> std::io::Result<Self> {
        Self::parse_fields(body.fields())
    }

    fn parse_fields<'a>(mut fields: impl FallibleIterator<Item = backend::ErrorField<'a>, Error = std::io::Error>) -> std::io::Result<Self> {
        let mut error = DbError {
            severity: String::new(),
            code: String::new(),
            message: String::new(),
            detail: None,
            position: None,
            internal_position: None,
        };
        while let Some(field) = fields.next()? {
            let value = String::from_utf8_lossy(field.value_bytes()).into_owned();
            match field.type_() {
                b'V' | b'S' if error.severity.is_empty() => error.severity = value,
                b'C' => error.code = value,
                b'M' => error.message = value,
                // 提示追加到细节后面：两段小字说明已经超出了这个面板的
                // 需要。
                b'D' => error.detail = Some(value),
                b'H' => error.detail = Some(match error.detail.take() {
                    Some(detail) => format!("{detail}\n{value}"),
                    None => value,
                }),
                b'P' => error.position = value.parse().ok(),
                b'p' => error.internal_position = value.parse().ok(),
                _ => {}
            }
        }
        Ok(error)
    }

    pub(crate) fn is_notice(&self) -> bool {
        !matches!(self.severity.as_str(), "ERROR" | "FATAL" | "PANIC")
    }

    pub(crate) fn to_app_error(&self) -> AppError {
        let kind = match self.code.as_str() {
            // query_canceled、admin_shutdown
            "57014" | "57P01" => "cancelled",
            code if code.starts_with("08") => "connection",
            code if code.starts_with("28") => "auth",
            _ => "database",
        };
        let mut error = AppError::new(kind, self.message.clone())
            .with_detail(self.detail.clone())
            .with_code(&self.code);
        // 执行器会把合并脚本里的位置映射到具体语句（并在那里丢弃它）；
        // 单独执行的语句保留位置，编辑器因此能指向出错的 token。函数内部
        // 抛出的错误两者都报：语句本身，以及语句里的那次调用。
        if self.internal_position.is_none() {
            if let Some(position) = self.position {
                error = error.with_position(position);
            }
        }
        error
    }
}

pub(crate) fn io_error(context: &str, err: impl std::fmt::Display) -> AppError {
    AppError::new("connection", context).detail(err.to_string())
}

pub(crate) fn db_error(body: backend::ErrorResponseBody) -> AppError {
    match DbError::parse(&body) {
        Ok(error) => error.to_app_error(),
        Err(err) => io_error("read_server_error", err),
    }
}
