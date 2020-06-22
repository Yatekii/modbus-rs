// #![no_std]

mod consts;
mod data;
mod error;
mod general;
mod modbus;
mod request;

#[cfg(feature = "atomic")]
use bbqueue::atomic::BBBuffer;
#[cfg(not(feature = "atomic"))]
use bbqueue::cm_mutex::BBBuffer;
use bbqueue::{ArrayLength, AutoReleaseGrantR, Consumer, Producer};
use core::{
    pin::Pin,
    task::{Context, Waker},
};
pub use data::CoilState;
pub use error::Error;
pub use futures::{task::Poll, Future};
pub use modbus::Modbus;
pub use request::Request;
