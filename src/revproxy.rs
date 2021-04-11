#![cfg(feature = "proxy")]
use openssl::ssl::{SslConnector, SslMethod};
use std::io;
use std::net::ToSocketAddrs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::conn;
use crate::logger;
use crate::status::Status;

pub async fn proxy(addr: String, u: url::Url, mut con: conn::Connection) -> Result<(), io::Error> {
    let p: Vec<&str> = u.path().trim_start_matches("/").splitn(2, "/").collect();
    if p.len() == 1 {
        logger::logger(con.peer_addr, Status::NotFound, u.as_str());
        con.send_status(Status::NotFound, None).await?;
        con.stream.shutdown().await?;
        return Ok(());
    }
    if p[1] == "" || p[1] == "/" {
        logger::logger(con.peer_addr, Status::NotFound, u.as_str());
        con.send_status(Status::NotFound, None).await?;
        con.stream.shutdown().await?;
        return Ok(());
    }
    let addr = addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::from(io::ErrorKind::AddrNotAvailable))?;

    let mut config = SslConnector::builder(SslMethod::tls()).unwrap().build().configure().unwrap().into_ssl("localhost").unwrap();
    config.set_verify(openssl::ssl::SslVerifyMode::NONE);

    let stream = match TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(_) => {
            logger::logger(con.peer_addr, Status::ProxyError, u.as_str());
            con.send_status(Status::ProxyError, None).await?;
            con.stream.shutdown().await?;
            return Ok(());
        }
    };

    let mut stream = tokio_openssl::SslStream::new(config, stream).unwrap();
    std::pin::Pin::new(&mut stream).connect().await.unwrap();
    /*
    let mut stream = match std::pin::Pin::new(&mut stream).connect().await.as_mut() {
        Ok(s) => s,
        Err(_) => {
            logger::logger(con.peer_addr, Status::ProxyError, u.as_str());
            con.send_status(Status::ProxyError, None).await?;
            return Ok(());
        }
    };
    */
    stream.write_all(p[1].as_bytes()).await?;
    stream.flush().await?;

    let mut buf = vec![];
    stream.read_to_end(&mut buf).await?;
    // let req = String::from_utf8(buf[..].to_vec()).unwrap();
    con.send_raw(&buf).await?;
    con.stream.shutdown().await?;
    Ok(())
}

pub async fn proxy_all(addr: &str, u: url::Url, mut con: conn::Connection) -> Result<(), io::Error> {
    let domain = addr.splitn(2, ':').next().unwrap();
    let mut config = SslConnector::builder(SslMethod::tls()).unwrap().build().configure().unwrap().into_ssl(domain).unwrap();
    config.set_verify(openssl::ssl::SslVerifyMode::NONE);
    
    // TCP handshake
    let stream = match TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(_) => {
            logger::logger(con.peer_addr, Status::ProxyError, u.as_str());
            con.send_status(Status::ProxyError, None).await?;
            con.stream.shutdown().await?;
            return Ok(());
        }
    };

    // TLS handshake with SNI
    let mut stream = tokio_openssl::SslStream::new(config, stream).unwrap();
    std::pin::Pin::new(&mut stream).connect().await.unwrap();
    /*
    let mut stream = match std::pin::Pin::new(&mut stream).connect().await {
        Ok(s) => s,
        Err(_) => {
            logger::logger(con.peer_addr, Status::ProxyError, u.as_str());
            con.send_status(Status::ProxyError, None).await?;
            return Ok(());
        }
    };
    */
    // send request: URL + CRLF
    stream.write_all(u.as_ref().as_bytes()).await?;
    stream.write_all(b"\r\n").await?;
    stream.flush().await?;

    // stream to client
    con.send_stream(&mut stream).await?;
    con.stream.shutdown().await?;
    Ok(())
}
