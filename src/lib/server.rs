#![allow(unreachable_code)]
use tokio::net::TcpListener;
use tokio::io::AsyncReadExt;
use openssl::ssl::SslAcceptor;
use openssl::error::ErrorStack;
use openssl::ssl::NameType;
//use futures_util::future::TryFutureExt;
use url::Url;
use std::io;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::pin::Pin;

use crate::config;
use crate::conn;
use crate::logger;
use crate::status::Status;
use crate::errors::{GemError, Result};

pub trait Handler: FnMut(conn::Connection, url::Url) -> Pin<Box<dyn Future<Output = Result> + Send>> + Send + Sync + Copy {}
impl<T> Handler for T
    where T: FnMut(conn::Connection, url::Url) -> Pin<Box<dyn Future<Output = Result> + Send>> + Send + Sync + Copy
{
}

pub fn force_boxed<T>(f: fn(conn::Connection, url::Url) -> T) -> impl Handler
where
    T: Future<Output = Result> + Send + Sync + 'static,
{
    move |a, b| Box::pin(f(a, b)) as _
}

pub struct Server {
    pub listener: Vec<TcpListener>,
    pub acceptor: SslAcceptor,
}

impl Server {
    pub async fn bind(addr: Vec<std::net::SocketAddr>,
        acceptor: fn(config::Config) -> std::result::Result<SslAcceptor, ErrorStack>,
        cfg: config::Config) -> io::Result<Server> 
    {
        if addr.len() == 1 {
            Ok(Server {
                listener: vec![TcpListener::bind(addr[0].to_owned()).await?],
                acceptor: acceptor(cfg)?,
            })
        } else {
            let mut listener: Vec<TcpListener> = Vec::new();
            for a in addr {
                listener.append(&mut vec![TcpListener::bind(a.to_owned()).await?]);
            }
            Ok(Server {
                listener,
                acceptor: acceptor(cfg)?,
            })
        }
    }

    pub async fn serve(self, cmap: HashMap<String, config::ServerCfg>, default: String,
        handler: impl Handler + 'static + Copy) -> Result
    {
        for listen in self.listener  {
            let cmap = cmap.clone();
            let default = default.clone();
            let listen = Arc::new(listen);
            let acceptor = Arc::new(self.acceptor.clone());
            tokio::spawn(async move {
                loop {
                    let (stream, peer_addr) = listen.accept().await?;
                    let local_addr = stream.local_addr().unwrap();
                    let acceptor = acceptor.clone();
                    let cmap = cmap.clone();
                    let default = default.clone();
                    let mut handler = handler.clone();

                    let ssl = openssl::ssl::Ssl::new(acceptor.context()).unwrap();
                    let mut stream = tokio_openssl::SslStream::new(ssl, stream).unwrap();

                    tokio::spawn(async move {
                        match Pin::new(&mut stream).accept().await {
                            Ok(s) => s,
                            Err(e) => {
                                log::error!("Error: {}",e);
                                return Ok(());
                            },
                        };
                        let srv = match stream.ssl().servername(NameType::HOST_NAME) {
                            Some(s) => match cmap.get(s) {
                                Some(ss) => ss,
                                None => cmap.get(&default).unwrap(),
                            },
                            None => cmap.get(&default).unwrap(),
                        }.to_owned();

                        let con = conn::Connection { stream, local_addr, peer_addr, srv };
                        let (con, url) = match get_request(con).await {
                            Ok((c, u)) => (c, u),
                            Err(_) => return Ok(()) as io::Result<()>,
                        };
                        
                        match handler(con, url).await {
                            Ok(o) => o,
                            Err(_) => return Ok(()) as io::Result<()>,
                        }

                        Ok(()) as io::Result<()>
                    });
                }
                Ok(()) as io::Result<()>
            });
        }
        tokio::signal::ctrl_c().await.expect("failed to listen for event");
        Ok(())
    }
}

pub async fn get_request(mut con: conn::Connection) -> Result<(conn::Connection, url::Url)> {
    let mut buffer = [0; 1024];
    let len = match tokio::time::timeout(tokio::time::Duration::from_secs(5), con.stream.read(&mut buffer)).await {
        Ok(result) => result.unwrap(),
        Err(e) => {
            logger::logger(con.peer_addr, Status::BadRequest, "");
            con.send_status(Status::BadRequest, None).await.map_err(|e| e.to_string())?;
            return Err(Box::new(e));
        }
    };
    let mut request = match String::from_utf8(buffer[..len].to_vec()) {
        Ok(request) => request,
        Err(e) => {
            logger::logger(con.peer_addr, Status::BadRequest, "");
            con.send_status(Status::BadRequest, None).await.map_err(|e| e.to_string())?;
            return Err(Box::new(e));
        }
    };
    if request.starts_with("//") {
        request = request.replacen("//", "gemini://", 1);
    }

    if request.ends_with("\n") {
        request.pop();
        if request.ends_with("\r") {
            request.pop();
        }
    }

    let url = match Url::parse(&request) {
        Ok(url) => url,
        Err(e) => {
            logger::logger(con.peer_addr, Status::BadRequest, &request);
            con.send_status(Status::BadRequest, None).await.map_err(|e| e.to_string())?;
            return Err(Box::new(e));
        }
    };

    match url.host_str() {
        Some(h) => {
            if con.srv.server.hostname.as_str() != h.to_lowercase() {
                logger::logger(con.peer_addr, Status::ProxyRequestRefused, &url.as_str());
                con.send_status(Status::ProxyRequestRefused, None).await.map_err(|e| e.to_string())?;
                return Err(Box::new(GemError("Wrong host".into())));
            }
        },
        None => {}
    }
    match url.port() {
        Some(p) => {
            if p != con.local_addr.port() {
                logger::logger(con.peer_addr, Status::ProxyRequestRefused, &url.as_str());
                con.send_status(Status::ProxyRequestRefused, None)
                    .await.map_err(|e| e.to_string())?;
            }
        }
        None => {}
    }
    if url.scheme() != "gemini" {
        logger::logger(con.peer_addr, Status::ProxyRequestRefused, &url.as_str());
        con.send_status(Status::ProxyRequestRefused, None).await.map_err(|e| e.to_string())?;
        return Err(Box::new(GemError("scheme not gemini".into())));
    }
    
    return Ok((con, url))
}