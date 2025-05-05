use thiserror::Error;

use crate::bencode;

#[derive(Debug, Error)]
pub enum Error {
    #[allow(clippy::enum_variant_names)]
    #[error("tracker error: {0}")]
    TrackerError(String),
    #[error("tracker EOF")]
    Eof,
    #[error("create socket IO error: {0}")]
    CreateSocket(#[source] std::io::Error),
    #[error("registrar IO error: {0}")]
    Registrar(#[source] std::io::Error),
    #[error("tracker connect IO error: {0}")]
    Connect(#[source] std::io::Error),
    #[error("tracker read IO error: {0}")]
    Read(#[source] std::io::Error),
    #[error("tracker write IO error: {0}")]
    Write(#[source] std::io::Error),
    #[error("tracker send_to IO error: {0}")]
    SendTo(#[source] std::io::Error),
    #[error("tracker timeout")]
    Timeout,
    #[error("DNS timeout")]
    DnsTimeout,
    #[error("invalid DNS")]
    DnsNotFound,
    #[error("DNS IO error: {0}")]
    DnsIo(#[source] std::io::Error),
    #[error("{0}: failed to parse {1} as URL: {2}")]
    UrlParse(&'static str, String, #[source] url::ParseError),
    #[error("unsupported URL scheme: {0}")]
    UrlUnsupportedScheme(String),
    #[error("URL {0} has no host")]
    UrlNoHost(String),
    #[error("URL {0} has no port")]
    UrlNoPort(String),
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("ualformed HTTP: {0}")]
    MalformedHttp(#[source] httparse::Error),
    #[error("redirect with no location")]
    RedirectNoLocation,
    #[error("response {0} is invalid bencode: {1}")]
    ResponseInvalidBencode(String, #[source] bencode::BError),
    #[error("response not dictionary")]
    ResponseNotDictionary,
    #[error("non-UTF-8 failure reason in response: {0}")]
    ResponseNonUtf8FailureReason(#[source] std::string::FromUtf8Error),
    #[error("no interval in response")]
    ResponseNoInterval,
    #[error("failed to parse error in UDP response: {0}")]
    UdpResponseInvalid(#[source] std::io::Error),
    #[error("connection failed")]
    Connection,
    #[error("bad state transition")]
    BadStateTransition,
}

pub type Result<T> = std::result::Result<T, Error>;
