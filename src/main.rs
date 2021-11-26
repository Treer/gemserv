#[macro_use]
extern crate serde_derive;

use std::io;
use std::net::ToSocketAddrs;

mod lib;
mod cgi;
mod config;
mod logger;
mod revproxy;
mod con_handler;

use lib::util;
use lib::conn;
use lib::status;
use lib::tls;
use lib::server;
use lib::errors;

#[tokio::main]
async fn main() -> errors::Result {
    let cfg = match config::Config::new().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Config error: {}", e);
            return Ok(());
        },
    };
    
    logger::init(&cfg.log)?;
    
    let cmap = cfg.to_map();
    let default = &cfg.server[0].hostname;
    println!("Serving {} vhosts", cfg.server.len());

    let mut addr: Vec<std::net::SocketAddr> = Vec::new();
    if cfg.host.is_some() && cfg.port.is_some() {
        addr.push(format!("{}:{}", &cfg.host.to_owned().unwrap(), &cfg.port.unwrap())
            .to_socket_addrs()?.next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::AddrNotAvailable))?);
    } else {
        match &cfg.interface {
            Some(i) => {
                for iface in i {
                    addr.push(iface
                        .to_socket_addrs()?.next()
                        .ok_or_else(|| io::Error::from(io::ErrorKind::AddrNotAvailable))?);
                } 
            },
            None => {},
        }
    }

    addr.sort_by(|a, b| a.port().cmp(&b.port()));
    addr.dedup();

    let server = server::Server::bind(addr, tls::acceptor_conf, cfg.clone()).await?;
    if let Err(e) = server.serve(cmap, default.to_string(), server::force_boxed(con_handler::handle_connection)).await {
            return Err(e)
    };
    return Ok(())
}
