//! Public error type. The message passes through [`crate::public_text`] at
//! construction so it is bounded, redacted and stripped of control characters
//! before it can reach any display or protocol surface.

use crate::Json;
use crate::text::public_text;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub code: String,
    pub message: String,
    pub field: Option<String>,
}

impl Error {
    pub fn new(code: impl Into<String>, message: impl AsRef<str>) -> Self {
        Error::with_field(code, message, None)
    }

    pub fn with_field(
        code: impl Into<String>,
        message: impl AsRef<str>,
        field: Option<&str>,
    ) -> Self {
        Error {
            code: code.into(),
            message: public_text(message.as_ref(), 4096),
            field: field.map(str::to_owned),
        }
    }

    pub fn json(&self) -> Json {
        let mut value = Json::object_from([
            ("code", Json::from(self.code.as_str())),
            ("message", Json::from(self.message.as_str())),
        ]);
        if let Some(field) = &self.field {
            value.insert("field", Json::from(field.as_str()));
        }
        value
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub fn fail<T>(code: &str, message: &str) -> Result<T> {
    Err(Error::new(code, message))
}
