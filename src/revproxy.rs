#![cfg(feature = "proxy")]
use std::convert::TryFrom;
use std::io;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use tokio_rustls::rustls;
use tokio_rustls::TlsConnector;

use crate::conn;
use crate::logger;
use crate::status::Status;
use crate::tls;

pub async fn proxy(addr: String, u: url::Url, mut con: conn::Connection) -> Result<(), io::Error> {
    let p: Vec<&str> = u.path().trim_start_matches("/").splitn(2, "/").collect();
    if p.len() == 1 {
        logger::logger(con.peer_addr, Status::NotFound, u.as_str());
        con.send_status(Status::NotFound, None).await?;
        return Ok(());
    }
    if p[1] == "" || p[1] == "/" {
        logger::logger(con.peer_addr, Status::NotFound, u.as_str());
        con.send_status(Status::NotFound, None).await?;
        return Ok(());
    }
    let domain = &addr;
    let addr = addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::from(io::ErrorKind::AddrNotAvailable))?;

    let config = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(tls::GeminiServerAuth))
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));

    let stream = TcpStream::connect(&addr).await?;

    let domain = rustls::ServerName::try_from(domain.as_str())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid dnsname"))?;

    let mut stream = connector.connect(domain, stream).await?;

    stream.write_all(p[1].as_bytes()).await?;
    stream.flush().await?;

    let mut buf = vec![];
    stream.read_to_end(&mut buf).await?;
    // let req = String::from_utf8(buf[..].to_vec()).unwrap();
    con.send_raw(&buf).await?;
    Ok(())
}

pub async fn proxy_all(
    addr: &str,
    u: url::Url,
    mut con: conn::Connection,
) -> Result<(), io::Error> {
    let domain = addr.splitn(2, ':').next().unwrap();

    let config = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(tls::GeminiServerAuth))
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));

    let stream = TcpStream::connect(&addr).await?;

    let domain = rustls::ServerName::try_from(domain)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid dnsname"))?;

    let mut stream = connector.connect(domain, stream).await?;

    // send request: URL + CRLF
    stream.write_all(u.as_ref().as_bytes()).await?;
    stream.write_all(b"\r\n").await?;
    stream.flush().await?;

    // stream to client
    con.send_stream(&mut stream).await?;
    Ok(())
}
