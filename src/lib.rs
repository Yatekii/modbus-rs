#[cfg(feature = "atomic")]
use bbqueue::atomic::{consts::*, BBBuffer};
#[cfg(not(feature = "atomic"))]
use bbqueue::cm_mutex::{consts::*, BBBuffer};
use bbqueue::{
    framed::{FrameConsumer, FrameGrantR, FrameGrantW, FrameProducer},
    ArrayLength,
};
use crc::{crc16, Hasher16};
use futures::{task::Poll, Future};
use nom::{bytes::streaming::take, *};
use scroll::Pread;
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

// #[derive(Clone)]
pub struct CoilStore<'a, S: ArrayLength<u8>>(FrameGrantR<'a, S>);

impl<'a, S: ArrayLength<u8>> CoilStore<'a, S> {
    fn into_iter(&'a self) -> CoilIterator<'a> {
        CoilIterator {
            current: 0,
            coils: &self.0,
        }
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

pub struct CoilIterator<'a> {
    current: usize,
    coils: &'a [u8],
}

impl<'a> Iterator for CoilIterator<'a> {
    type Item = CoilState;
    fn next(&mut self) -> Option<Self::Item> {
        let byte = self.current / 8;
        let bit = self.current % 8;
        if byte < self.coils.len() {
            self.current += 1;
            Some(if self.coils[byte] >> bit & 1 == 1 {
                CoilState::On
            } else {
                CoilState::Off
            })
        } else {
            None
        }
    }
}

pub enum Request<'a, S: ArrayLength<u8>> {
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
        coils: CoilStore<'a, S>,
    },
    SetRegisters {
        address: u16,
        count: u16,
        registers: &'a [u16],
    },
}

pub struct Modbus<'a, S: ArrayLength<u8>> {
    producer: FrameProducer<'a, S>,
    consumer: FrameConsumer<'a, S>,
    waker: Option<Waker>,
    current_frame: Option<Result<Request<'a, S>, Error>>,
    needed: usize,
    frame_position: usize,
}

impl<'a, S: ArrayLength<u8> + 'a> Modbus<'a, S> {
    pub fn new(bb: &'a BBBuffer<S>) -> Modbus<'a, S> {
        let (producer, consumer) = bb.try_split_framed().unwrap();

        Modbus {
            producer,
            consumer,
            waker: None,
            current_frame: None,
            needed: 0,
            frame_position: 0,
        }
    }

    fn crc_valid(data: &[u8], crc: u16) -> bool {
        let mut digest = crc16::Digest::new(crc::crc16::USB);
        digest.write(data);
        digest.sum16() == crc
    }

    fn parse_read_request<'b>(input: &'b [u8]) -> IResult<&'b [u8], (u16, u16, u16)> {
        let (input, address) = take(2usize)(input)?;
        let (input, count) = take(2usize)(input)?;
        let (input, crc) = take(2usize)(input)?;

        let address: u16 = address.pread(0).unwrap();
        let count: u16 = count.pread(0).unwrap();
        let crc: u16 = crc.pread(0).unwrap();

        Ok((input, (address, count, crc)))
    }

    fn parse_frame(&mut self, wgr: FrameGrantW<S>) -> Result<Request<'a, S>, Error> {
        // let (producer, consumer) = self.buffer.try_split_framed().unwrap();

        // let rgr = consumer.read().unwrap();
        // let data = &rgr[..];

        let data = &wgr[..];

        let (input, _slave_address) = take(1usize)(data)?;
        let (input, function) = take(1usize)(input)?;

        match function[0] {
            1 => {
                let (_input, (address, count, _crc)) = Self::parse_read_request(input)?;
                wgr.commit(0);
                Ok(Request::ReadCoil { address, count })
            }
            2 => {
                let (_input, (address, count, _crc)) = Self::parse_read_request(input)?;
                wgr.commit(0);
                Ok(Request::ReadInput { address, count })
            }
            3 => {
                let (_input, (address, count, _crc)) = Self::parse_read_request(input)?;
                wgr.commit(0);
                Ok(Request::ReadOutputRegisters { address, count })
            }
            4 => {
                let (_input, (address, count, _crc)) = Self::parse_read_request(input)?;
                wgr.commit(0);
                Ok(Request::ReadInputRegisters { address, count })
            }
            5 => {
                // TODO:
                let (_input, (address, count, _crc)) = Self::parse_read_request(input)?;
                wgr.commit(0);
                Ok(Request::ReadCoil { address, count })
            }
            6 => {
                // TODO:
                let (_input, (address, count, _crc)) = Self::parse_read_request(input)?;
                wgr.commit(0);
                Ok(Request::ReadCoil { address, count })
            }
            15 => {
                // TODO:
                let (_input, (address, count, _crc)) = Self::parse_read_request(input)?;
                wgr.commit(0);
                let rgr = self.consumer.read().unwrap();
                Ok(Request::SetCoils {
                    address,
                    count,
                    coils: CoilStore(rgr),
                })
            }
            16 => {
                // TODO:
                let (_input, (address, count, _crc)) = Self::parse_read_request(input)?;
                wgr.commit(0);
                Ok(Request::ReadCoil { address, count })
            }
            f => Err(Error::UnknownFunction(f)),
        }
    }

    /// Call this in the data received interrupt.
    pub fn on_data_received(&'a mut self, data: &[u8]) {
        // Add the newly received data to the grant.
        let mut wgr = self.producer.grant(256).unwrap();
        wgr[self.frame_position..self.frame_position + data.len()].clone_from_slice(&data);

        if wgr.len() >= self.needed {
            self.needed = 0;
        } else {
            return;
        }

        match self.parse_frame(wgr) {
            // If the frame is not complete yet, wait for more bytes.
            Err(Error::Parse(Err::Incomplete(needed))) => match needed {
                // Do nothing if the parser has no info about the number of required bytes.
                Needed::Unknown => (),
                // Remember the number of the needed bytes until we can parse the frame if the number is known.
                // TODO: determine if this actually ever hits or if we can spare those few instructions.
                Needed::Size(number) => self.needed = number,
            },
            // If we succeed to parse the frame, commit the bytes to the buffer and reset the position in the frame to 0.
            // Then wake the waker.
            Ok(frame) => {
                self.current_frame = Some(Ok(frame));
                if let Some(waker) = self.waker.take() {
                    self.frame_position = 0;
                    waker.wake();
                }
            }
            // If we fail to parse the frame, do NOT commit the bytes to the buffer and reset the position in the frame to 0.
            // This way we can reuse the frame space.
            // Then wake the waker.
            Err(err) => {
                self.current_frame = Some(Err(err));
                if let Some(waker) = self.waker.take() {
                    waker.wake();
                    self.frame_position = 0;
                }
            }
        }
    }

    pub async fn next(&'a mut self) -> Result<Request<'a, S>, Error> {
        struct RequestFuture<'a, S: ArrayLength<u8>> {
            bus: &'a mut Modbus<'a, S>,
        }

        impl<'a, S: ArrayLength<u8>> Future for RequestFuture<'a, S> {
            type Output = Result<Request<'a, S>, Error>;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                if let Some(frame) = self.bus.current_frame.take() {
                    Poll::Ready(frame)
                } else {
                    self.bus.waker = Some(cx.waker().clone());
                    Poll::Pending
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
    Parse(nom::Err<nom::error::ErrorKind>),
    #[error("Unknown function: {0}")]
    UnknownFunction(u8),
}

impl From<nom::Err<(&[u8], nom::error::ErrorKind)>> for Error {
    fn from(e: nom::Err<(&[u8], nom::error::ErrorKind)>) -> Self {
        Error::Parse(match e {
            nom::Err::Error((_r, e)) => nom::Err::Error(e),
            nom::Err::Failure((_r, e)) => nom::Err::Failure(e),
            nom::Err::Incomplete(e) => nom::Err::Incomplete(e),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test() {
        println!("Hello, world!");
    }
}
