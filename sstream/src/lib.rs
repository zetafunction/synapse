use std::io::{self, Read};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::Arc;

use net2::{TcpBuilder, TcpStreamExt};

const EINPROGRESS: i32 = 115;

/// Nonblocking Secure TcpStream implementation.
pub struct SStream {
    conn: SConn,
    fd: i32,
}

enum SConn {
    Plain(TcpStream),
    #[allow(clippy::upper_case_acronyms)]
    SSLC(rustls::StreamOwned<rustls::ClientConnection, TcpStream>),
    #[allow(clippy::upper_case_acronyms)]
    SSLS(rustls::StreamOwned<rustls::ServerConnection, TcpStream>),
}

impl SStream {
    pub fn new_v6(host: Option<String>) -> io::Result<SStream> {
        let sock = TcpBuilder::new_v6()?.to_tcp_stream()?;
        SStream::new(sock, host)
    }

    pub fn new_v4(host: Option<String>) -> io::Result<SStream> {
        let sock = TcpBuilder::new_v4()?.to_tcp_stream()?;
        SStream::new(sock, host)
    }

    fn new(sock: TcpStream, host: Option<String>) -> io::Result<SStream> {
        sock.set_nonblocking(true)?;
        let fd = sock.as_raw_fd();
        Ok(match host {
            Some(h) => {
                let root_store = rustls::RootCertStore {
                    roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
                };
                let config = rustls::ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_no_client_auth();
                let dns_name = rustls::pki_types::DnsName::try_from_str(&h)
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "invalid host string used")
                    })?
                    .to_owned();
                let conn = rustls::ClientConnection::new(
                    Arc::new(config),
                    rustls::pki_types::ServerName::DnsName(dns_name),
                )
                .map_err(std::io::Error::other)?;
                SStream {
                    conn: SConn::SSLC(rustls::StreamOwned::new(conn, sock)),
                    fd,
                }
            }
            None => SStream {
                conn: SConn::Plain(sock),
                fd,
            },
        })
    }

    pub fn connect(&mut self, addr: SocketAddr) -> io::Result<()> {
        match &mut self.conn {
            SConn::Plain(sock) | SConn::SSLC(rustls::StreamOwned { sock, .. }) => {
                if let Err(e) = sock.connect(addr) {
                    if Some(EINPROGRESS) != e.raw_os_error() {
                        return Err(e);
                    }
                }
                Ok(())
            }
            SConn::SSLS { .. } => unreachable!("Server side TLS connect"),
        }
    }

    pub fn from_plain(sock: TcpStream) -> io::Result<SStream> {
        sock.set_nonblocking(true)?;
        let fd = sock.as_raw_fd();
        Ok(SStream {
            conn: SConn::Plain(sock),
            fd,
        })
    }

    pub fn from_ssl(sock: TcpStream, config: &Arc<rustls::ServerConfig>) -> io::Result<SStream> {
        sock.set_nonblocking(true)?;
        let fd = sock.as_raw_fd();
        let conn = rustls::ServerConnection::new(config.clone()).map_err(std::io::Error::other)?;
        Ok(SStream {
            conn: SConn::SSLS(rustls::StreamOwned::new(conn, sock)),
            fd,
        })
    }

    pub fn get_stream(&self) -> &TcpStream {
        match &self.conn {
            SConn::Plain(sock) => sock,
            SConn::SSLC(rustls::StreamOwned { sock, .. }) => sock,
            SConn::SSLS(rustls::StreamOwned { sock, .. }) => sock,
        }
    }

    fn read_(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.conn {
            SConn::Plain(sock) => sock.read(buf),
            SConn::SSLC(stream) => {
                // Attempt to call complete_io as many times as necessary
                // to complete handshaking. Once handshaking is complete
                // session.read should begin returning results which we
                // can then use. complete_io returning 0, 0 indicates that
                // EOF has been reached, but we still need to read out
                // the remaining bytes, propagating EOF. Prior to this
                // reading 0 bytes simply indicates the TLS session buffer
                // has no data
                loop {
                    match stream.conn.complete_io(&mut stream.sock)? {
                        (0, 0) => {
                            return stream.read(buf);
                        }
                        _ => {
                            let res = stream.read(buf)?;
                            if res > 0 {
                                return Ok(res);
                            }
                        }
                    }
                }
            }
            SConn::SSLS(stream) => loop {
                match stream.conn.complete_io(&mut stream.sock)? {
                    (0, 0) => {
                        return stream.read(buf);
                    }
                    _ => {
                        let res = stream.read(buf)?;
                        if res > 0 {
                            return Ok(res);
                        }
                    }
                }
            },
        }
    }
}

impl io::Read for SStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.read_(buf) {
            Ok(n) => Ok(n),
            Err(e) => {
                if e.kind() == io::ErrorKind::ConnectionAborted {
                    return Ok(0);
                }
                Err(e)
            }
        }
    }
}

impl io::Write for SStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &mut self.conn {
            SConn::Plain(stream) => stream.write(buf),
            SConn::SSLC(stream) => {
                let result = stream.write(buf);
                stream.conn.complete_io(&mut stream.sock)?;
                result
            }
            SConn::SSLS(stream) => {
                let result = stream.write(buf);
                stream.conn.complete_io(&mut stream.sock)?;
                result
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.conn {
            SConn::Plain(stream) => stream.flush(),
            SConn::SSLC(stream) => {
                stream.flush()?;
                stream.sock.flush()
            }
            SConn::SSLS(stream) => {
                stream.flush()?;
                stream.sock.flush()
            }
        }
    }
}

impl AsRawFd for SStream {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

// TODO: Add tests
#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
