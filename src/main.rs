#![allow(unused_imports)]
#![allow(dead_code)]
#[macro_use]
extern crate serde_derive;

use futures_util::future::TryFutureExt;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::fs::File;
use std::io::{self, BufReader, BufRead};
use std::io::prelude::*;
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::env;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::prelude::*;
use tokio::runtime;
use tokio_rustls::rustls::internal::pemfile::{certs, pkcs8_private_keys};
use tokio_rustls::rustls::{Certificate, NoClientAuth, PrivateKey, ServerConfig};
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;
use url::Url;
use chrono::{DateTime, Utc};
use mime_guess;
use mime;

mod cgi;
mod config;
mod status;
mod tls;
mod conn;

fn get_mime(path: &PathBuf) -> String {
    let mut mime = "text/gemini";
    let m: mime::Mime;

    let ext = match path.extension() {
        Some(p) => p.to_str().unwrap(),
        None => return mime.to_string(),
    };
    if ext != "gemini" {
        m = mime_guess::from_ext(ext).first().unwrap();
        mime = m.essence_str();
    } 
    mime.to_string()
}

async fn get_binary(mut con: conn::Connection, path:PathBuf, meta: String) -> io::Result<()> {
    let fd = File::open(path)?;
    let mut reader = BufReader::with_capacity(1024*1024,fd);
    con.send_status(status::Status::Success, &meta).await?;
    loop {
        let len = {
            let buf = reader.fill_buf()?;
            con.send_raw(buf).await?;
            buf.len()
        };
        if len == 0 {
            break
        }
        reader.consume(len);
    }
    Ok(())
}

fn get_content(path: PathBuf, u: url::Url) -> Result<String, io::Error> {
    let meta = fs::metadata(&path).expect("Unable to read metadata");
    if meta.is_file() {
        return Ok(std::fs::read_to_string(path).expect("Unable to read file"));
    }

    let mut list = String::from("# Directory Listing\r\n\r\n");
    list.push_str(&format!("Path: {}\r\n\r\n", u.path()));
    // needs work
    for file in fs::read_dir(&path)? {
        if let Ok(file) = file {
            let m = file.metadata()?;
            let perm = m.permissions();
            if perm.mode() & 0o0444 != 0o0444 {
                continue
            }
            let file = file.path();
            let p = file.strip_prefix(&path).unwrap();
            if m.is_dir() {
                list.push_str(&format!("=> {}/ {}/\r\n", p.display(), p.display()));
            } else {
                list.push_str(&format!("=> {} {}\r\n", p.display(), p.display()));
            }
        }
    }
    return Ok(list);
}

async fn handle_connection(mut con: conn::Connection) -> Result<(), io::Error> {
    let now: DateTime<Utc> = Utc::now();
    println!("{} New Connection: {}", now, con.peer_addr);
    let mut buffer = [0; 1024];
    con.stream.read(&mut buffer).await?;
    let request = match String::from_utf8(buffer[..].to_vec()) {
        Ok(request) => request,
        Err(_) => {
            println!("Bad Request");
            con.send_status(status::Status::BadRequest, "Bad Request!\r\n").await?;
            return Ok(())
        }
    };
    println!("Request: {}", request);

    let url = match Url::parse(&request) {
        Ok(url) => url,
        Err(_) => { con.send_status(status::Status::BadRequest, "Bad Request!\r\n").await?;
                return Ok(())
        }
    };

    if Some(con.hostname.as_str()) != url.host_str() {
        con.send_status(status::Status::ProxyRequestRefused, "Url doesn't match certificate!").await?;
        return Ok(());
    }

    if url.scheme() != "gemini" {
        con.send_status(
            status::Status::ProxyRequestRefused,
            "Not a gemini scheme!\r\n",
        )
        .await?;
        return Ok(());
    }

    let mut path = PathBuf::new();

    if url.path().starts_with("/~") && con.usrdir == true{
        let usr = url.path().trim_start_matches("/~");
        let usr: Vec<&str> = usr.splitn(2, "/").collect();
        path.push("/home/");
        if usr.len() == 2 {
            path.push(format!("{}/{}/{}", usr[0], "public_gemini", usr[1]));
        } else {
            path.push(format!("{}/{}/",usr[0], "public_gemini"));
        }
    } else {
        path.push(&con.dir);
        if url.path() != "" || url.path() != "/" {
            path.push(url.path().trim_start_matches("/"));
        }

    }

    if !path.exists() {
        con.send_status(status::Status::NotFound, "Not found!\r\n").await?;
        return Ok(());
    }

    let mut meta = fs::metadata(&path).expect("Unable to read metadata");
    let mut perm = meta.permissions();

    if meta.is_dir() {
        if !url.path().ends_with("/") {
            con.send_status(
                status::Status::RedirectPermanent,
                format!("{}/\r\n", url).as_str(),
            )
            .await?;
            return Ok(());
        }
        if path.join("index.gemini").exists() {
            path.push("index.gemini");
            meta = fs::metadata(&path).expect("Unable to read metadata");
            perm = meta.permissions();
        }
    }

    // add timeout
    if con.cgi.trim_end_matches("/") == path.parent().unwrap().to_str().unwrap() {
        if perm.mode() & 0o0111 == 0o0111 {
        cgi::cgi(con, path, url).await?;
        return Ok(());
        } else {
            con.send_status(
                status::Status::CGIError, "CGI Error!\r\n").await?;
            return Ok(());
        }
    }

    if perm.mode() & 0o0444 != 0o0444 {
        con.send_status(
            status::Status::NotFound, "Not Found!\r\n").await?;
        return Ok(());
    }

    let mime = get_mime(&path);
    if !mime.starts_with("text/") {
        get_binary(con, path, mime).await?;
        return Ok(());
    }
    let content = get_content(path, url)?;
    con.send_body(
        status::Status::Success,
        mime.as_str(),
        Some(content),
    )
    .await?;

    Ok(())
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        println!("Please run with the path to the config file.");
        return Ok(());
    }
    let p = Path::new(&args[1]);
    if !p.exists() {
        println!("Config file doesn't exist");
        return Ok(());
    }
    let cfg = config::Config::new(&p)?;
    println!("Serving {} vhosts", cfg.server.len());

    let addr = format!("{}:{}", cfg.host, cfg.port);
    addr.to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::from(io::ErrorKind::AddrNotAvailable))?;

    let mut runtime = runtime::Builder::new()
        .threaded_scheduler()
        .enable_io()
        .build()?;

    let handle = runtime.handle().clone();
    let config = tls::get_tls_config(cfg.clone());

    let acceptor = TlsAcceptor::from(Arc::new(config));

    let fut = async {
        let mut listener = TcpListener::bind(&addr).await?;
        loop {
            let (stream, peer_addr) = listener.accept().await?;
            let acceptor = acceptor.clone();
            let cfg = cfg.clone();

            let fut = async move {
                let mut stream = acceptor.accept(stream).await?;
                let (_, sni) = TlsStream::get_mut(&mut stream);
                let sni = sni.get_sni_hostname();
                let mut dir = String::new();
                let mut cgi = String::new();
                let mut hostname = String::new();
                let mut usrdir: bool = false;

                for server in &cfg.server {
                    if Some(server.hostname.as_str()) == sni {
                        hostname = sni.unwrap().to_string();
                        dir = server.dir.to_string();
                        if server.cgi.is_some() {
                            cgi = server.cgi.as_ref().unwrap().to_string();
                        }
                        if server.usrdir.is_some() {
                            usrdir = server.usrdir.unwrap();
                        }
                        break;
                    }
                }

                let con = conn::Connection {
                    stream,
                    peer_addr,
                    hostname,
                    dir,
                    cgi,
                    usrdir,
                };
                handle_connection(con).await?;

                Ok(()) as io::Result<()>
            };

            handle.spawn(fut.unwrap_or_else(|err| eprintln!("{:?}", err)));
        }
    };

    runtime.block_on(fut)
}
