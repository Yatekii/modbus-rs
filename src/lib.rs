#![no_std]
mod consts;

#[cfg(feature = "atomic")]
use bbqueue::atomic::BBBuffer;
#[cfg(not(feature = "atomic"))]
use bbqueue::cm_mutex::BBBuffer;
use bbqueue::{ArrayLength, Consumer, GrantR, Producer};
use core::{
    pin::Pin,
    task::{Context, Waker},
};
use futures::{task::Poll, Future};
use scroll::{Pread, BE};

pub struct Frame {}
pub struct Response {}

#[derive(Debug, PartialEq)]
pub enum CoilState {
    On = 0xFF00,
    Off = 0x0000,
}

#[derive(Debug, PartialEq)]
pub struct CoilStore<'a, S: ArrayLength<u8>>(GrantR<'a, S>);

impl<'a, S: ArrayLength<u8>> CoilStore<'a, S> {
    pub fn iter(&'a self) -> CoilIterator<'a> {
        CoilIterator {
            current: 0,
            coils: &self.0,
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl<'a, S: ArrayLength<u8>> IntoIterator for &'a CoilStore<'a, S> {
    type Item = CoilState;
    type IntoIter = CoilIterator<'a>;

    fn into_iter(self) -> Self::IntoIter {
        CoilIterator {
            current: 0,
            coils: &self.0,
        }
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
    producer: Producer<'a, S>,
    consumer: Consumer<'a, S>,
    waker: Option<Waker>,
    needed_bytes: Option<usize>,
}

impl<'a, S: ArrayLength<u8> + 'a> Modbus<'a, S> {
    pub fn new(bb: &'a BBBuffer<S>) -> Modbus<'a, S> {
        let (producer, consumer) = bb.try_split().unwrap();

        Modbus {
            producer,
            consumer,
            waker: None,
            needed_bytes: None,
        }
    }

    /// Returns true if the CRC matches the data.
    ///
    /// Expects the last two bytes of the data to be the CRC.
    fn crc_valid(data: &[u8]) -> bool {
        crc16::State::<crc16::MODBUS>::calculate(data) == 0
    }

    fn parse_read_request<'b>(data: &'b [u8]) -> (u16, u16) {
        let address: u16 = data.pread_with(0, BE).unwrap();
        let count: u16 = data.pread_with(2, BE).unwrap();

        (address, count)
    }

    fn parse_frame(&mut self, rgr: GrantR<S>) -> Result<Request<'a, S>, Error> {
        let len = parse_request_len(&rgr[..]).unwrap().unwrap();
        let crc_valid = Self::crc_valid(&rgr[..len]);
        if !crc_valid {
            return Err(Error::Crc);
        }
        let _slave_address = rgr[0];
        let function_id = rgr[1];
        let data = &rgr[2..len];

        let r = match function_id {
            1 => {
                let (address, count) = Self::parse_read_request(data);
                Ok(Request::ReadCoil { address, count })
            }
            2 => {
                let (address, count) = Self::parse_read_request(data);
                Ok(Request::ReadInput { address, count })
            }
            3 => {
                let (address, count) = Self::parse_read_request(data);
                Ok(Request::ReadOutputRegisters { address, count })
            }
            4 => {
                let (address, count) = Self::parse_read_request(data);
                Ok(Request::ReadInputRegisters { address, count })
            }
            5 => {
                // TODO:
                let (address, count) = Self::parse_read_request(data);
                Ok(Request::ReadCoil { address, count })
            }
            6 => {
                // TODO:
                let (address, count) = Self::parse_read_request(data);
                Ok(Request::ReadCoil { address, count })
            }
            15 => {
                // TODO:
                let (address, count) = Self::parse_read_request(data);
                let rgr = self.consumer.read().unwrap();
                Ok(Request::SetCoils {
                    address,
                    count,
                    coils: CoilStore(rgr),
                })
            }
            16 => {
                // TODO:
                let (address, count) = Self::parse_read_request(data);
                Ok(Request::ReadCoil { address, count })
            }
            f => Err(Error::UnknownFunction(f)),
        };

        r
    }

    /// Call this in the data received interrupt.
    pub fn on_data_received(&mut self, data: &[u8]) {
        // Get a grant that is as large as the size of the received data.
        let mut wgr = self.producer.grant_exact(data.len()).unwrap();

        // Copy the data from the receive buffer into the bbqueue.
        wgr.clone_from_slice(&data);

        // Make sure we commit the stored bytes.
        wgr.commit(data.len());

        // TODO: Handle wraparound.
        let rgr = self.consumer.read().unwrap();
        if let Some(needed_bytes) = self.needed_bytes {
            // If we don't need anymore bytes, call the waker.
            if rgr.len() >= needed_bytes {
                if let Some(waker) = self.waker.take() {
                    waker.wake();
                }
            }
        } else {
            // If we do not know the amount of bytes required, make sure we still wake the poller
            // such that it can check for the required amount.
            if let Some(waker) = self.waker.take() {
                waker.wake();
            }
        }
    }

    pub async fn next(&mut self) -> Result<Request<'_, S>, Error> {
        struct RequestFuture<'a: 'b, 'b, S: ArrayLength<u8>> {
            bus: &'b mut Modbus<'a, S>,
        }

        impl<'a: 'b, 'b, S: ArrayLength<u8> + 'a> Future for RequestFuture<'a, 'b, S> {
            type Output = Result<Request<'a, S>, Error>;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                match self.bus.needed_bytes {
                    Some(0) => {
                        // We don't require anymore bytes to parse the next frame.
                        // So we reset everything and parse the frame.

                        // Reset needed bytes to unknown for the next frame.
                        self.bus.needed_bytes = None;
                        // Read the stored bytes.
                        let rgr = self.bus.consumer.read().unwrap();
                        // Parse and return the frame from the stored bytes.
                        Poll::Ready(self.bus.parse_frame(rgr))
                    }
                    None => {
                        self.bus.waker = Some(cx.waker().clone());
                        // Read the stored bytes.
                        let rgr = self.bus.consumer.read().unwrap();
                        match parse_request_len(&rgr[..]) {
                            Ok(len) => {
                                // We store the number of needed bytes, whether it is known or unknown (None, Some(len)).
                                self.bus.needed_bytes = len;
                                // Instantly check if we can yield a new frame!
                                if let Some(needed_bytes) = self.bus.needed_bytes {
                                    // If we don't need anymore bytes, call the waker.
                                    if rgr.len() >= needed_bytes {
                                        // Parse and return the frame from the stored bytes.
                                        return Poll::Ready(self.bus.parse_frame(rgr));
                                    }
                                }
                            }
                            // If an unknown function is encountered we cannot parse the frame length
                            // and thus we cannot parse the entire frame.
                            // For now we just panic here.
                            // TODO: Implement a recovery mechanism. Maybe a timeout?
                            Err(_e) => unimplemented!("An unknown function id was encountered; How do we handle this properly?")
                        }
                        Poll::Pending
                    }
                    Some(_) => {
                        // Wait on for more bytes.
                        Poll::Pending
                    }
                }
            }
        }

        RequestFuture { bus: self }.await
    }
}

/// Returns the complete length of a request dataframe including slave ID and CRC.
/// The returned Result is always Ok except if the function code was unknown.
/// If there was not enough databytes received yet, Ok(None) is returned.
fn parse_request_len(data: &[u8]) -> Result<Option<usize>, Error> {
    // If the packet is not at least two bytes long, we cannot determine the function code
    // as well as the packet length, so we instanly return None, signaling that we await more bytes.
    if data.len() < 2 {
        return Ok(None);
    }
    let fn_code = data[1];
    Ok(match fn_code {
        consts::READ_COIL..=consts::SET_REGISTER => Some(8),
        consts::SET_COILS | consts::SET_REGISTERS => {
            if data.len() > 6 {
                Some(9 + data[6] as usize)
            } else {
                // incomplete frame
                None
            }
        }
        _ => {
            return Err(Error::UnknownFunction(fn_code));
        }
    })
}

#[derive(Debug, PartialEq)]
pub enum Error {
    Crc,
    UnknownFunction(u8),
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
