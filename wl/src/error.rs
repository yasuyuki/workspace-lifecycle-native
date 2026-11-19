use std::fmt;
use std::path::Path;

#[derive(Debug)]
pub struct Error(String);

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(message: impl Into<String>) -> Self { Self(message.into()) }
    pub fn io(path: &Path, error: std::io::Error) -> Self { Self(format!("{}: {error}", path.display())) }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}

impl<E: std::error::Error> From<E> for Error {
    fn from(error: E) -> Self { Self(error.to_string()) }
}
