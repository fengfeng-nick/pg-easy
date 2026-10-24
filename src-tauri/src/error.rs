use std::fmt;

use serde::Serialize;

/// A detail composed here around a token the UI translates (`lib/errors.ts`),
/// with the values its `{placeholders}` take.
#[derive(Debug, Clone, Serialize)]
pub struct Translated {
    token: String,
    args: Vec<String>,
}

impl Translated {
    pub fn new(token: &str) -> Self {
        Self {
            token: token.into(),
            args: Vec::new(),
        }
    }

    pub fn args(mut self, args: impl IntoIterator<Item = String>) -> Self {
        self.args = args.into_iter().collect();
        self
    }

    /// The token's arguments, e.g. a NOTICE's severity and text.
    pub fn arg_values(&self) -> &[String] {
        &self.args
    }
}

/// Everything the UI needs to explain a failure, split into a stable
/// machine-readable kind and a human-facing message.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub kind: &'static str,
    /// A stable token the UI translates (see `lib/errors.ts`), or text from
    /// the PostgreSQL server, which the UI shows as-is.
    pub message: String,
    /// Values for `message`'s `{placeholders}`, in the order they appear.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Free text (a server or I/O message), or a `Translated` token for a
    /// detail this app composed around one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
    /// 0-based index of the statement that failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statement_index: Option<usize>,
    /// Transaction state after the failure, so the UI can warn about aborted
    /// transactions without polling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_status: Option<&'static str>,
    /// Statements that completed before the failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results: Option<Vec<crate::protocol::StatementResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results_duration_ms: Option<f64>,
    /// Rows the failing statement had already received.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<crate::protocol::StatementResult>,
}

impl AppError {
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            args: None,
            detail: None,
            code: None,
            position: None,
            statement_index: None,
            tx_status: None,
            results: None,
            results_duration_ms: None,
            rows: None,
        }
    }

    pub fn with_detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail.map(serde_json::Value::String);
        self
    }

    /// Detail for an I/O failure that names the file involved: `{path}: {error}`.
    pub fn with_io_path(self, path: &std::path::Path, error: &dyn std::error::Error) -> Self {
        self.with_token_detail("io_path", [path.display().to_string(), error.to_string()])
    }

    /// Placeholder values for `message`'s `{placeholders}`.
    pub fn with_args<I, A>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = A>,
        A: ToString,
    {
        self.args = Some(args.into_iter().map(|arg| arg.to_string()).collect());
        self
    }

    /// A detail composed here around a token the UI translates, e.g. the
    /// file path in "could not read the root certificate".
    pub fn with_token_detail<I, A>(self, token: &str, args: I) -> Self
    where
        I: IntoIterator<Item = A>,
        A: ToString,
    {
        let args: Vec<String> = args.into_iter().map(|arg| arg.to_string()).collect();
        self.with_translated_detail(Translated::new(token).args(args))
    }

    pub fn with_translated_detail(mut self, translated: Translated) -> Self {
        self.detail = serde_json::to_value(translated).ok();
        self
    }

    pub fn with_code(mut self, code: &str) -> Self {
        if !code.is_empty() {
            self.code = Some(code.to_string());
        }
        self
    }

    pub fn with_position(mut self, position: usize) -> Self {
        self.position = Some(position);
        self
    }

    pub fn with_statement(mut self, index: usize) -> Self {
        self.statement_index = Some(index);
        self
    }

    pub fn with_tx_status(mut self, status: &'static str) -> Self {
        self.tx_status = Some(status);
        self
    }

    pub fn with_rows(mut self, partial: crate::protocol::StatementResult) -> Self {
        self.rows = Some(partial);
        self
    }

    pub fn with_results(
        mut self,
        results: Vec<crate::protocol::StatementResult>,
        duration_ms: f64,
    ) -> Self {
        self.results = Some(results);
        self.results_duration_ms = Some(duration_ms);
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(serde_json::Value::String(detail.into()));
        self
    }

    pub fn unexpected(err: impl fmt::Display) -> Self {
        Self::new("internal", "something_went_wrong").detail(err.to_string())
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}
