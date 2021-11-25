use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub struct GemError(pub String);

impl fmt::Display for GemError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for GemError {}
