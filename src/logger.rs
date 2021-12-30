use crate::lib::errors;
use crate::status;
use log::{info, warn};
use std::net::SocketAddr;

pub fn init(loglev: &Option<String>) -> errors::Result {
    let loglev = match loglev {
        None => log::LevelFilter::Info,
        Some(l) => match l.as_str() {
            "error" => log::LevelFilter::Error,
            "warn" => log::LevelFilter::Warn,
            "info" => log::LevelFilter::Info,
            _ => {
                return Err(Box::new(errors::GemError(
                    "Incorrect log level in config file.".to_string(),
                )));
            }
        },
    };
    simple_logger::SimpleLogger::new()
        .with_level(loglev)
        .with_utc_timestamps()
        .init()?;
    Ok(())
}

pub fn logger(addr: SocketAddr, stat: status::Status, req: &str) {
    match stat as u8 {
        20..=29 => info!("remote={} status={} request={}", addr, stat as u8, req),
        _ => warn!("remote={} status={} request={}", addr, stat as u8, req),
    }
}
