use std::io;

use crate::ProtocolError;

#[derive(Debug)]
pub enum ReceiveError {
    Io(io::Error),
    Protocol(ProtocolError),
}

impl std::fmt::Display for ReceiveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Protocol(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ReceiveError {}
