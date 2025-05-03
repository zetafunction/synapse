use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("read IO error: {0}")]
    Read(#[source] std::io::Error),
    #[error("write IO error: {0}")]
    Write(#[source] std::io::Error),
    #[error("client connection completed")]
    Complete,
    #[error("invalid utf-8: {0}")]
    InvalidUtf8(#[source] std::string::FromUtf8Error),
    #[error("failed to decode payload: {0}")]
    BadPayload(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;
