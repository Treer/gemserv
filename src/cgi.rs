#![cfg(any(feature = "cgi", feature = "scgi"))]
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;

#[cfg(feature = "cgi")]
use std::path::PathBuf;
#[cfg(feature = "cgi")]
use tokio::process::Command;

use tokio_rustls::rustls::ServerConnection;

#[cfg(feature = "scgi")]
use std::net::ToSocketAddrs;
#[cfg(feature = "scgi")]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(feature = "scgi")]
use tokio::net::TcpStream;

use crate::config;
use crate::conn;
use crate::logger;
use crate::status::Status;
use crate::util;

#[cfg(any(feature = "cgi", feature = "scgi"))]
fn envs(
    peer_addr: SocketAddr,
    session: &ServerConnection,
    srv: &config::ServerCfg,
    url: &url::Url,
) -> HashMap<String, String> {
    let mut envs = HashMap::new();
    envs.insert("GATEWAY_INTERFACE".to_string(), "CGI/1.1".to_string());
    envs.insert("GEMINI_URL".to_string(), url.to_string());
    envs.insert(
        "SERVER_NAME".to_string(),
        url.host_str().unwrap().to_string(),
    );
    envs.insert("SERVER_PROTOCOL".to_string(), "GEMINI".to_string());
    let addr = peer_addr.ip().to_string();
    envs.insert("REMOTE_ADDR".to_string(), addr.clone());
    envs.insert("REMOTE_HOST".to_string(), addr);
    let port = peer_addr.port().to_string();
    envs.insert("REMOTE_PORT".to_string(), port);
    envs.insert(
        "SERVER_SOFTWARE".to_string(),
        env!("CARGO_PKG_NAME").to_string(),
    );

    if let Some(q) = url.query() {
        envs.insert("QUERY_STRING".to_string(), q.to_string());
    }

    if let Some(cert) = session.peer_certificates() {
        let cert = tokio_rustls::rustls::Certificate::as_ref(&cert[0]);
        if let Ok((_, x509)) = x509_parser::parse_x509_certificate(cert) {
            let user = x509
                .subject()
                .iter_common_name()
                .next()
                .and_then(|cn| cn.as_str().ok())
                .unwrap();

            envs.insert("AUTH_TYPE".to_string(), "Certificate".to_string());
            envs.insert("REMOTE_USER".to_string(), user.to_string());
            envs.insert("TLS_CLIENT_HASH".to_string(), util::fingerhex(cert));
        }
    }

    match &srv.server.cgienv {
        Some(c) => {
            for (k, v) in c.iter() {
                envs.insert(k.clone(), v.clone());
            }
        }
        None => {}
    }
    envs
}

#[cfg(any(feature = "cgi", feature = "scgi"))]
fn check(byt: u8, peer_addr: SocketAddr, u: &url::Url) -> bool {
    match byt {
        49 => {
            logger::logger(peer_addr, Status::Input, u.as_str());
        }
        50 => {
            logger::logger(peer_addr, Status::Success, u.as_str());
        }
        51..=54 => {}
        _ => {
            logger::logger(peer_addr, Status::CGIError, u.as_str());
            return false;
        }
    }
    true
}

#[cfg(feature = "cgi")]
pub async fn cgi(
    con: &mut conn::Connection,
    path: PathBuf,
    url: &url::Url,
    script_name: String,
    path_info: String,
) -> Result<(), io::Error> {
    let (_, session) = con.stream.get_ref();
    let mut envs = envs(con.peer_addr, session, &con.srv, url);
    envs.insert("SCRIPT_NAME".into(), script_name);
    envs.insert("PATH_INFO".into(), path_info);

    if let Some(p) = path.parent() {
        std::env::set_current_dir(p)?;
    }

    let cmd = Command::new(path.to_str().unwrap())
        .env_clear()
        .envs(&envs)
        .output();

    let cmd = match tokio::time::timeout(tokio::time::Duration::from_secs(5), cmd).await {
        Ok(c) => match c {
            Ok(cc) => cc,

            Err(_) => {
                logger::logger(con.peer_addr, Status::CGIError, url.as_str());
                con.send_status(Status::CGIError, None).await?;
                return Ok(());
            }
        },
        Err(_) => {
            logger::logger(con.peer_addr, Status::CGIError, url.as_str());
            con.send_status(Status::CGIError, None).await?;
            return Ok(());
        }
    };

    if !cmd.status.success() {
        logger::logger(con.peer_addr, Status::CGIError, url.as_str());
        con.send_status(Status::CGIError, None).await?;
        return Ok(());
    }
    let cmd = cmd.stdout;
    if !check(cmd[0], con.peer_addr, url) {
        con.send_status(Status::CGIError, None).await?;
        return Ok(());
    }

    con.send_raw(&cmd).await?;
    Ok(())
}

#[cfg(feature = "scgi")]
pub async fn scgi(addr: String, u: url::Url, mut con: conn::Connection) -> Result<(), io::Error> {
    let addr = addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::from(io::ErrorKind::AddrNotAvailable))?;

    let mut stream = match TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(_) => {
            logger::logger(con.peer_addr, Status::CGIError, u.as_str());
            con.send_status(Status::CGIError, None).await?;
            return Ok(());
        }
    };
    let (_, session) = con.stream.get_ref();
    let envs = envs(con.peer_addr, session, &con.srv, &u);
    let len = 0usize;
    let mut byt = format!(
        "CONTENT_LENGTH\x00{}\x00SCGI\x001\x00
        RQUEST_METHOD\x00POST\x00REQUEST_URI\x00{}\x00",
        len,
        u.path()
    );
    for (k, v) in envs.iter() {
        byt.push_str(&format!("{}\x00{}\x00", k, v));
    }
    byt = byt.len().to_string() + ":" + &byt + ",";

    stream.write_all(byt.as_bytes()).await?;
    stream.flush().await?;

    let mut buf = vec![];
    if let Err(_) = tokio::time::timeout(
        tokio::time::Duration::from_secs(5),
        stream.read_to_end(&mut buf),
    )
    .await
    {
        logger::logger(con.peer_addr, Status::CGIError, u.as_str());
        con.send_status(Status::CGIError, None).await?;
        return Ok(());
    }
    let req = String::from_utf8_lossy(&buf[..]);
    if !check(req.as_bytes()[0], con.peer_addr, &u) {
        con.send_status(Status::CGIError, None).await?;
        return Ok(());
    }

    con.send_raw(req.as_bytes()).await?;
    Ok(())
}
