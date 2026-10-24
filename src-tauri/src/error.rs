use std::fmt;

use serde::Serialize;

/// 在这里围绕界面会翻译的 token 组合出的细节（`lib/errors.ts`），连同它的
/// `{placeholders}` 要填的值。
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

    /// token 的参数，例如 NOTICE 的级别和文本。
    pub fn arg_values(&self) -> &[String] {
        &self.args
    }
}

/// 界面解释一次失败所需的全部信息，拆成稳定的机器可读类型和人读的消息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub kind: &'static str,
    /// 界面会翻译的稳定 token（见 `lib/errors.ts`），或来自 PostgreSQL
    /// 服务器的文本，后者界面原样显示。
    pub message: String,
    /// `message` 的 `{placeholders}` 值，按出现顺序排列。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// 自由文本（服务器或 I/O 消息），或本应用围绕某个细节组合出的
    /// `Translated` token。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
    /// 失败语句从 0 开始的索引。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statement_index: Option<usize>,
    /// 失败之后的事务状态，界面据此提示事务已中止，不必轮询。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_status: Option<&'static str>,
    /// 失败之前已完成的语句。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results: Option<Vec<crate::protocol::StatementResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results_duration_ms: Option<f64>,
    /// 失败语句此前已经收到的行。
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

    /// 会点出涉及文件的 I/O 失败细节：`{path}: {error}`。
    pub fn with_io_path(self, path: &std::path::Path, error: &dyn std::error::Error) -> Self {
        self.with_token_detail("io_path", [path.display().to_string(), error.to_string()])
    }

    /// `message` 的 `{placeholders}` 占位值。
    pub fn with_args<I, A>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = A>,
        A: ToString,
    {
        self.args = Some(args.into_iter().map(|arg| arg.to_string()).collect());
        self
    }

    /// 在这里围绕界面会翻译的 token 组合出的细节，例如“读取根证书失败”里的
    /// 文件路径。
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
