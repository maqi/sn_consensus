use core::fmt::Debug;
use thiserror::Error;

use super::{abba, vcbc, NodeId};

#[derive(Error, Debug)]
pub enum Error {
    #[error("encoding/decoding error {0:?}")]
    Encoding(#[from] bincode::Error),
    #[error("vcbc error {0:?}")]
    Vcbc(#[from] vcbc::error::Error),
    #[error("abba error {0:?}")]
    Abba(#[from] abba::error::Error),
    #[error("invalid message {0}")]
    InvalidMessage(String),
    #[error("generic error {0}")]
    Generic(String),
    #[error("unknown node id {0}")]
    UnknownNodeId(NodeId),
}

pub type Result<T> = std::result::Result<T, Error>;
