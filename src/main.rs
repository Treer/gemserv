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

use tokio::signal::unix;
use tokio::sync::watch;

async fn run(mut recv: watch::Receiver<bool>) -> errors::Result {
    loop {
        let cfg = match config::Config::new().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Config error: {}", e);
                return Ok(());
            }
        };

        // This will error because log init only wants to be called once.
        // On reload it will allow going from higher to lower logging levels
        // however trying to go from a lower lever to higher won't change.
        if let Err(_) = logger::init(&cfg.log) {}

        let cmap = cfg.to_map();
        log::info!("Serving {} vhosts", cfg.server.len());

        let mut addr: Vec<std::net::SocketAddr> = Vec::new();
        if let Some(i) = &cfg.interface {
            addr.append(&mut i.to_owned());
        }

        let server = server::Server::bind(addr, tls_acceptor_conf, cfg).await?;
        if let Err(e) = server
            .serve(
                cmap,
                server::force_boxed(con_handler::handle_connection),
                recv.clone(),
            )
            .await
        {
            return Err(e);
        };
        recv.changed().await?;
    }
}

async fn signal_select(send: watch::Sender<bool>) -> errors::Result {
    let mut hangup = unix::signal(unix::SignalKind::hangup())?;
    let mut sigterm = unix::signal(unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                log::info!("Received ctrl-c shutting down!");
                send.send(false)?;
                std::process::exit(0);
            },
            _ = sigterm.recv() => {
                log::info!("Received SIGTERM shutting down");
                send.send(false)?;
                std::process::exit(0);
            },
            _ = hangup.recv() => {
                log::info!("Received SIGHUP reloading config.");
                send.send(false)?;
            }
        }
    }
}

#[tokio::main]
async fn main() -> errors::Result {
    let (send, recv) = watch::channel(true);
    tokio::spawn(async move {
        signal_select(send).await?;
        return Ok(()) as errors::Result;
    });
    run(recv).await?;
    Ok(())
}
