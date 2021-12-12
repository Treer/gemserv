#[macro_use]
extern crate serde_derive;

#[cfg(any(feature = "cgi", feature = "scgi"))]
mod cgi;
mod con_handler;
mod config;
mod lib;
mod logger;
#[cfg(feature = "proxy")]
mod revproxy;

use lib::conn;
use lib::errors;
use lib::server;
use lib::status;
use lib::tls::{self, tls_acceptor_conf};
use lib::util;

#[tokio::main]
async fn main() -> errors::Result {
    let cfg = match config::Config::new().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Config error: {}", e);
            return Ok(());
        }
    };

    logger::init(&cfg.log)?;

    let cmap = cfg.to_map();
    println!("Serving {} vhosts", cfg.server.len());

    let mut addr: Vec<std::net::SocketAddr> = Vec::new();
    if let Some(i) = &cfg.interface {
        addr.append(&mut i.to_owned());
    }

    let server = server::Server::bind(addr, tls_acceptor_conf, cfg).await?;
    if let Err(e) = server
        .serve(
            cmap,
            server::force_boxed(con_handler::handle_connection),
        )
        .await
    {
        return Err(e);
    };
    Ok(())
}
