use crate::status;
use log::{info, warn};
use std::net::SocketAddr;

pub fn init(loglev: &Option<String>) {
    let loglev = match loglev {
        None => log::Level::Info,
        Some(l) => {
            match l.as_str() {
                "error" => log::Level::Error,
                "warn" => log::Level::Warn,
                "info" => log::Level::Info,
                _ => {
                    eprintln!("Incorrect log level in config file.");
                    std::process::exit(1);
               },
            }
        },
    };
    simple_logger::init_with_level(loglev).unwrap();
}

pub fn logger(addr: SocketAddr, stat: status::Status, req: &str) {
    match stat as u8 {
        20..=29 => info!("remote={} status={} request={}", addr, stat as u8, req),
        _ => warn!("remote={} status={} request={}", addr, stat as u8, req),
    }
}
