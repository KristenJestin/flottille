use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Store(rusqlite::Error),
    Json(serde_json::Error),
    /// A refusal, said in a sentence the user can act on.
    Refused(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(error) => error.fmt(f),
            Error::Store(error) => write!(f, "store: {error}"),
            Error::Json(error) => write!(f, "json: {error}"),
            Error::Refused(sentence) => f.write_str(sentence),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Error {
        Error::Io(error)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Error {
        Error::Store(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Error {
        Error::Json(error)
    }
}
