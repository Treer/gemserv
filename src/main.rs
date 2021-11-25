#[macro_use]
extern crate serde_derive;

use std::env;
use std::io;
use std::net::ToSocketAddrs;
use std::path::Path;

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

type Result<T=()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::main]
async fn main() -> Result {
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

    let cfg = match config::Config::new(&p) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Config error: {}", e);
            return Ok(());
        },
    };
    
    logger::init(&cfg.log);
    
    let cmap = cfg.to_map();
    let default = &cfg.server[0].hostname;
    println!("Serving {} vhosts", cfg.server.len());

    let addr = format!("{}:{}", cfg.host, cfg.port);
    addr.to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::from(io::ErrorKind::AddrNotAvailable))?;

    let server = server::Server::bind(addr, tls::acceptor_conf, cfg.clone()).await?;
    if let Err(e) = server.serve(cmap, default.to_string(), server::force_boxed(con_handler::handle_connection)).await {
            return Err(e)
    };
    return Ok(())
}
