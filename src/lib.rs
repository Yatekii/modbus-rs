#[cfg(feature = "atomic")]
use bbqueue::atomic::{consts::*, BBBuffer};
#[cfg(not(feature = "atomic"))]
use bbqueue::cm_mutex::{consts::*, BBBuffer};
use bbqueue::{
    framed::{FrameConsumer, FrameGrantR, FrameGrantW, FrameProducer},
    ArrayLength,
};
use futures::{task::Poll, Future};
use nom::{bytes::streaming::take, *};
use scroll::{Pread, BE, LE};
use std::{
    pin::Pin,
    task::{Context, Waker},
};
use thiserror::Error;

pub struct Frame {}
// pub struct Request {}
pub struct Response {}

#[derive(Debug, PartialEq)]
pub enum CoilState {
    On = 0xFF00,
    Off = 0x0000,
}

#[derive(Debug, PartialEq)]
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

#[derive(Debug, PartialEq)]
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
    frame_size: usize,
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
            frame_size: 0,
        }
    }

    fn crc_valid(data: &[u8], crc: u16) -> bool {
        crc16::State::<crc16::MODBUS>::calculate(data) == 0
    }

    fn parse_read_request<'b>(input: &'b [u8]) -> IResult<&'b [u8], (u16, u16, u16)> {
        let (input, address) = take(2usize)(input)?;
        let (input, count) = take(2usize)(input)?;
        let (input, crc) = take(2usize)(input)?;

        let address: u16 = address.pread_with(0, BE).unwrap();
        let count: u16 = count.pread_with(0, BE).unwrap();
        let crc: u16 = crc.pread_with(0, LE).unwrap();

        Ok((input, (address, count, crc)))
    }

    fn parse_frame(&mut self, wgr: FrameGrantW<S>) -> Result<Request<'a, S>, Error> {
        // let (producer, consumer) = self.buffer.try_split_framed().unwrap();

        // let rgr = consumer.read().unwrap();
        // let data = &rgr[..];

        let data = &wgr[..self.frame_size];

        let (input, _slave_address) = take(1usize)(data)?;
        let (input, function) = take(1usize)(input)?;

        let r = match function[0] {
            1 => {
                let (_input, (address, count, crc)) = Self::parse_read_request(input)?;
                let crc_valid = Self::crc_valid(&data[..8], crc);
                if !crc_valid {
                    return Err(Error::Crc);
                }
                wgr.commit(6);
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
        };

        r
    }

    /// Call this in the data received interrupt.
    pub fn on_data_received(&mut self, data: &[u8]) {
        // Add the newly received data to the grant.
        let mut wgr = self.producer.grant(256).unwrap();
        wgr[self.frame_size..self.frame_size + data.len()].clone_from_slice(&data);
        self.frame_size += data.len();

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
                    self.frame_size = 0;
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
                    self.frame_size = 0;
                }
            }
        }
    }

    pub async fn next(&mut self) -> Result<Request<'_, S>, Error> {
        struct RequestFuture<'a: 'b, 'b, S: ArrayLength<u8>> {
            bus: &'b mut Modbus<'a, S>,
        }

        impl<'a: 'b, 'b, S: ArrayLength<u8>> Future for RequestFuture<'a, 'b, S> {
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

#[derive(Error, Debug, PartialEq)]
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
    use super::{Error, Modbus, Request};
    use bbqueue::{atomic::consts::U2048, BBBuffer};

    #[tokio::test]
    async fn fn1_crc_correct() {
        let bb = BBBuffer::<U2048>::new();
        let mut modbus = super::Modbus::new(&bb);

        let data = [0x11, 0x01, 0x00, 0x13, 0x00, 0x25, 0x0E, 0x84];
        let address: u16 = 0x0013;
        let count: u16 = 0x0025;

        modbus.on_data_received(&data);
        assert_eq!(
            modbus.next().await,
            Ok(Request::ReadCoil { address, count })
        );
    }

    #[tokio::test]
    async fn fn1_crc_fail() {
        let bb = BBBuffer::<U2048>::new();
        let mut modbus = Modbus::new(&bb);

        let data = [0x11, 0x01, 0x00, 0x13, 0x00, 0x25, 0x0E, 0x85];

        modbus.on_data_received(&data);
        assert_eq!(modbus.next().await, Err(Error::Crc));
    }

    #[tokio::test]
    async fn fn1_data_in_2_steps() {
        let bb = BBBuffer::<U2048>::new();
        let mut modbus = super::Modbus::new(&bb);

        let data = [0x11, 0x01, 0x00, 0x13];
        let address: u16 = 0x0013;
        let count: u16 = 0x0025;

        modbus.on_data_received(&data);
        let data = [0x00, 0x25, 0x0E, 0x84];
        modbus.on_data_received(&data);
        assert_eq!(
            modbus.next().await,
            Ok(Request::ReadCoil { address, count })
        );
    }

    #[tokio::test]
    #[ignore]
    async fn fn1_2_futures_data_in_2_steps() {
        let bb = BBBuffer::<U2048>::new();
        let mut modbus = super::Modbus::new(&bb);

        let data = [0x11, 0x01, 0x00, 0x13];
        let address: u16 = 0x0013;
        let count: u16 = 0x0025;

        let data = [0x11, 0x01, 0x00, 0x13];
        modbus.on_data_received(&data);
        let data = [0x00, 0x25, 0x0E, 0x84];
        modbus.on_data_received(&data);

        let data = [0x11, 0x01, 0x00, 0x13];
        modbus.on_data_received(&data);
        let data = [0x00, 0x25, 0x0E, 0x84];
        modbus.on_data_received(&data);

        assert_eq!(
            modbus.next().await,
            Ok(Request::ReadCoil { address, count })
        );
        assert_eq!(
            modbus.next().await,
            Ok(Request::ReadCoil { address, count })
        );
    }

    #[test]
    fn fn2() {
        let data = [0x11, 0x02, 0x00, 0xC4, 0x00, 0x16, 0xBA, 0xA9];
        let slave_address = 0x11;
        let fn_code = 0x02;
        let address = 0x00C4;
        let count = 0x0016;
        let crc = 0xBAA9;
    }

    #[test]
    fn fn3() {
        let data = [0x11, 0x03, 0x00, 0x6B, 0x00, 0x03, 0x76, 0x87];
        let slave_address = 0x11;
        let fn_code = 0x03;
        let address = 0x006B;
        let count = 0x0003;
        let crc = 0x7687;
    }

    #[test]
    fn fn4() {
        let data = [0x11, 0x04, 0x00, 0x08, 0x00, 0x01, 0xB2, 0x98];
        let slave_address = 0x11;
        let fn_code = 0x04;
        let address = 0x0008;
        let count = 0x0001;
        let crc = 0xB298;
    }

    #[test]
    fn fn5_on() {
        let data = [0x11, 0x05, 0x00, 0xAC, 0xFF, 0x00, 0x4E, 0x8B];
        let slave_address = 0x11;
        let fn_code = 0x05;
        let address = 0x00AC;
        let status = true;
        let crc = 0x4E8B;
    }

    #[test]
    fn fn5_off() {
        let data = [0x11, 0x05, 0x00, 0xAC, 0x00, 0xFF, 0x4E, 0x8B];
        let slave_address = 0x11;
        let fn_code = 0x05;
        let address = 0x00AC;
        let status = false;
        let crc = 0x4E8B;
    }

    // #[test]
    // fn fn6() {
    //     let data = [0x11, 0x06, 0x00, 0x01 0003 9A9B];
    //     let slave_address = 0x11;
    //     let fn_code = 0x06;
    //     let address = 0x0001;
    //     let count = 0x0025;
    //     let crc = 0x9A9B;
    // }

    // #[test]
    // fn fn15() {
    //     let data = [11 0F 0013 000A 02 CD01 BF0B];
    //     let slave_address = 0x11;
    //     let fn_code = 0x0F;
    //     let address = 0x0013;
    //     let count = 0x0025;
    //     let crc = 0xBF0B;
    // }

    // #[test]
    // fn fn16() {
    //     let data = [11 10 0001 0002 04 000A 0102 C6F0];
    //     let slave_address = 0x11;
    //     let fn_code = 0x10;
    //     let address = 0x0013;
    //     let count = 0x0025;
    //     let crc = 0x0E84;
    // }
}
