//! Types and traits for handling incoming body data.

mod data;
mod from_data;

pub use self::data::Data;
pub use self::data::DataStream;
pub use self::from_data::{FromData, Outcome};
