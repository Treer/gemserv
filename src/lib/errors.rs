use std::error::Error;
use std::fmt;

pub type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
pub struct GemError(pub String);

impl fmt::Display for GemError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for GemError {}
