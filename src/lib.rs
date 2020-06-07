#[cfg(feature = "atomic")]
use bbqueue::atomic::{consts::*, BBBuffer};
#[cfg(not(feature = "atomic"))]
use bbqueue::cm_mutex::{consts::*, BBBuffer};
use bbqueue::{framed::FrameGrantR, ArrayLength};
use crc::{crc16, Hasher16};
use futures::{stream::Stream, task::Poll, Future};
use nom::{bytes::streaming::take, *};
use scroll::{ctx, Pread, LE};
use std::{
    pin::Pin,
    task::{Context, Waker},
};
use thiserror::Error;

pub struct Frame {}
// pub struct Request {}
pub struct Response {}

pub enum CoilState {
    On = 0xFF00,
    Off = 0x0000,
}

#[derive(Clone)]
pub struct CoilStore<'a>(FrameGrantR<'a, U256>);

impl<'a> CoilStore<'a> {
    fn iter(&self) -> CoilIterator<'a> {
        CoilIterator {
            current: 0,
            coils: self.clone(),
        }
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

pub struct CoilIterator<'a> {
    current: usize,
    coils: CoilStore<'a>,
}

impl<'a> Iterator for CoilIterator<'a> {
    type Item = CoilState;
    fn next(&mut self) -> Option<Self::Item> {
        let byte = self.current / 8;
        let bit = self.current % 8;
        if byte < self.coils.len() {
            self.current += 1;
            Some(if self.coils.0[byte] >> bit & 1 == 1 {
                CoilState::On
            } else {
                CoilState::Off
            })
        } else {
            None
        }
    }
}

pub enum Request<'a> {
    ReadCoil {
        address: u16,
        count: u16,
    },
    ReadInput {
        address: u16,
        count: u16,
    },
    ReadOutputRegisters {
        address: u16,
        count: u16,
    },
    ReadInputRegisters {
        address: u16,
        count: u16,
    },
    SetCoil {
        address: u16,
        status: CoilState,
    },
    SetRegister {
        address: u16,
        value: u16,
    },
    SetCoils {
        address: u16,
        count: u16,
        coils: CoilStore<'a>,
    },
    SetRegisters {
        address: u16,
        count: u16,
        registers: &'a [u16],
    },
}

pub struct Modbus<S: ArrayLength<u8>> {
    buffer: BBBuffer<S>,
    waker: Option<Waker>,
    frame_ready: bool,
    needed: usize,
}

impl<S: ArrayLength<u8>> Modbus<S> {
    pub fn new() -> Modbus<S> {
        let bb: BBBuffer<S> = BBBuffer::new();

        Modbus {
            buffer: bb,
            waker: None,
            frame_ready: false,
            needed: 0,
        }
    }

    fn parse_read_request(&self, data: &[u8], input: &[u8]) -> Result<(u16, u16), Error> {
        let (input, address) = take(2usize)(input)?;
        let (input, count) = take(2usize)(input)?;

        // Check the CRC.
        let (input, crc) = take(2usize)(input)?;
        let mut digest = crc16::Digest::new(crc::crc16::USB);
        digest.write(&data[..data.len() - 2]);
        if digest.sum16() != crc.pread(0).unwrap() {
            return Err(Error::Crc);
        }

        let address: u16 = address.pread(0).unwrap();
        let count: u16 = count.pread(0).unwrap();

        Ok((address, count))
    }

    fn parse_frame(&mut self, data: &[u8]) -> Result<Request, Error> {
        let (producer, consumer) = self.buffer.try_split_framed().unwrap();

        let rgr = consumer.read().unwrap();
        rgr

        let (input, _slave_address) = take(1usize)(data)?;
        let (input, function) = take(1usize)(input)?;

        match function[0] {
            1 => {
                let (address, count) = self.parse_read_request(data, input)?;
                Ok(Request::ReadCoil { address, count })
            }
            2 => {
                let (address, count) = self.parse_read_request(data, input)?;
                Ok(Request::ReadInput { address, count })
            }
            3 => {
                let (address, count) = self.parse_read_request(data, input)?;
                Ok(Request::ReadOutputRegisters { address, count })
            }
            4 => {
                let (address, count) = self.parse_read_request(data, input)?;
                Ok(Request::ReadInputRegisters { address, count })
            }
            5 => {
                // TODO:
                let (address, count) = self.parse_read_request(data, input)?;
                Ok(Request::ReadCoil { address, count })
            }
            6 => {
                // TODO:
                let (address, count) = self.parse_read_request(data, input)?;
                Ok(Request::ReadCoil { address, count })
            }
            15 => {
                // TODO:
                let (address, count) = self.parse_read_request(data, input)?;
                Ok(Request::ReadCoil { address, count })
            }
            16 => {
                // TODO:
                let (address, count) = self.parse_read_request(data, input)?;
                Ok(Request::ReadCoil { address, count })
            }
            f => Err(Error::UnknownFunction(f)),
        }
    }

    /// Call this in the data received interrupt.
    pub fn on_data_received(&mut self, data: &[u8]) {
        if data.len() >= self.needed {
            self.needed = 0;
        } else {
            return;
        }

        match self.parse_frame(data) {
            Err(Error::Parse(Err::Incomplete(needed))) => match needed {
                Needed::Unknown => (),
                Needed::Size(size) => self.needed = size,
            },
            Ok(_) | Err(_) => {
                self.frame_ready = true;
                if let Some(waker) = self.waker.take() {
                    waker.wake();
                }
            }
        }
    }

    pub async fn next(&mut self) -> Result<Request<'_>, Error> {
        struct RequestFuture<'a, S: ArrayLength<u8>> {
            bus: &'a mut Modbus<S>,
        }

        impl<'a, S: ArrayLength<u8>> Future for RequestFuture<'a, S> {
            type Output = Result<Request<'a>, Error>;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                if !self.bus.frame_ready {
                    self.bus.waker = Some(cx.waker().clone());
                    Poll::Pending
                } else {
                    Poll::Ready(self.bus.parse_frame(&[]))
                }
            }
        }

        RequestFuture { bus: self }.await
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("CRC could not be validated")]
    Crc,
    #[error("Message could not be parsed")]
    Parse(#[from] nom::Err<()>),
    #[error("Unknown function: {0}")]
    UnknownFunction(u8),
}

#[cfg(test)]
mod tests {
    #[test]
    fn test() {
        println!("Hello, world!");
    }
}
