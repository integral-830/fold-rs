use std::io;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, StorageError>;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("Key is too large: size={size} bytes, max={max} bytes")]
    KeyTooLarge { size: usize, max: usize },

    #[error("Value is too large: size={size} bytes, max={max} bytes")]
    ValueTooLarge { size: usize, max: usize },
}
