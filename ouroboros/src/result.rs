use std::result;

use crate::error::Error;

pub type Result<T> = result::Result<T, Error>;
