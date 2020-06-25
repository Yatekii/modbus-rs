#![no_std]

mod consts;
mod data;
mod error;
mod general;
mod modbus;
mod request;

pub use data::CoilState;
pub use error::Error;
pub use futures::{task::Poll, Future};
pub use modbus::Modbus;
pub use request::{Request, RequestFrame};
