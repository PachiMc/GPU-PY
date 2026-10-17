//! Error único con el mensaje ya traducido, listo para mostrar al usuario
//! (equivale a `BackendError` + `LedError` del código Python).

use thiserror::Error;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("{0}")]
pub struct HwError(pub String);

impl HwError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

pub type HwResult<T> = Result<T, HwError>;
