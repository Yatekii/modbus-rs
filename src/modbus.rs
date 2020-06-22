#[cfg(feature = "atomic")]
use bbqueue::atomic::BBBuffer;
#[cfg(not(feature = "atomic"))]
use bbqueue::cm_mutex::BBBuffer;

use crate::error::Error;
use crate::request::RequestFrame;
use bbqueue::{ArrayLength, Consumer, Producer};
use core::{
    pin::Pin,
    task::{Context, Waker},
};
use futures::{task::Poll, Future};

pub struct Modbus<'a, S: ArrayLength<u8>> {
    producer: Producer<'a, S>,
    consumer: Consumer<'a, S>,
    waker: Option<Waker>,
    needed_bytes: Option<usize>,
}

impl<'a, S: ArrayLength<u8> + 'a> Modbus<'a, S> {
    pub fn new(bb: &'a BBBuffer<S>) -> Modbus<'a, S> {
        let (producer, consumer) = bb.try_split().unwrap_or_else(|_| panic!());

        Modbus {
            producer,
            consumer,
            waker: None,
            needed_bytes: None,
        }
    }

    /// Call this in the data received interrupt.
    pub fn on_data_received(&mut self, data: &[u8]) {
        // Get a grant that is as large as the size of the received data.
        let mut wgr = self
            .producer
            .grant_exact(data.len())
            .unwrap_or_else(|_| panic!());

        // Copy the data from the receive buffer into the bbqueue.
        wgr.clone_from_slice(&data);

        // Make sure we commit the stored bytes.
        wgr.commit(data.len());

        // TODO: Handle wraparound.
        let rgr = self.consumer.read().unwrap_or_else(|_| panic!());
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

    pub async fn next(&mut self) -> Result<RequestFrame<'_, S>, Error> {
        struct RequestFuture<'a: 'b, 'b, S: ArrayLength<u8>> {
            bus: &'b mut Modbus<'a, S>,
        }

        impl<'a: 'b, 'b, S: ArrayLength<u8> + 'a> Future for RequestFuture<'a, 'b, S> {
            type Output = Result<RequestFrame<'a, S>, Error>;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                match self.bus.needed_bytes {
                    Some(frame_len) => {
                        // Read the stored bytes.
                        let rgr = self.bus.consumer.read().unwrap_or_else(|_| panic!());
                        let mut rgr = rgr.into_auto_release();
                        rgr.to_release(frame_len);

                        if rgr.len() >= frame_len {
                            // We don't require anymore bytes to parse the next frame.
                            // So we reset everything and parse the frame.

                            // Reset needed bytes to unknown for the next frame.
                            self.bus.needed_bytes = None;
                            // Parse and return the frame from the stored bytes.
                            Poll::Ready(RequestFrame::parse_frame(rgr, frame_len))
                        } else {
                            // Wait on for more bytes.
                            Poll::Pending
                        }
                    }
                    None => {
                        self.bus.waker = Some(cx.waker().clone());
                        // Read the stored bytes.
                        let rgr = self.bus.consumer.read().unwrap_or_else(|_| panic!());
                        match RequestFrame::<S>::parse_request_len(&rgr[..]) {
                            Ok(len) => {
                                // We store the number of needed bytes, whether it is known or unknown (None, Some(len)).
                                self.bus.needed_bytes = len;
                                // Instantly check if we can yield a new frame!
                                if let Some(frame_len) = self.bus.needed_bytes {
                                    // If we don't need anymore bytes, call the waker.
                                    if rgr.len() >= frame_len {
                                        self.bus.needed_bytes = None;
                                        // Parse and return the frame from the stored bytes.
                                        let mut rgr = rgr.into_auto_release();
                                        rgr.to_release(frame_len);
                                        return Poll::Ready(RequestFrame::parse_frame(rgr, frame_len));
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
                }
            }
        }

        RequestFuture { bus: self }.await
    }
}

#[cfg(test)]
mod tests {
    use crate::{CoilState, Error, Modbus, Request, RequestFrame};
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
            Ok(RequestFrame {
                slave_id: 0x11,
                request: Request::ReadCoil { address, count }
            })
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
            Ok(RequestFrame {
                slave_id: 0x11,
                request: Request::ReadCoil { address, count }
            })
        );
    }

    #[tokio::test]
    async fn fn1_2_futures_data_in_2_steps() {
        let bb = BBBuffer::<U2048>::new();
        let mut modbus = super::Modbus::new(&bb);

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
            Ok(RequestFrame {
                slave_id: 0x11,
                request: Request::ReadCoil { address, count }
            })
        );
        assert_eq!(
            modbus.next().await,
            Ok(RequestFrame {
                slave_id: 0x11,
                request: Request::ReadCoil { address, count }
            })
        );
    }

    #[tokio::test]
    async fn fn2() {
        let bb = BBBuffer::<U2048>::new();
        let mut modbus = super::Modbus::new(&bb);

        let data = [0x11, 0x02, 0x00, 0xC4, 0x00, 0x16, 0xBA, 0xA9];
        let address = 0x00C4;
        let count = 0x0016;

        modbus.on_data_received(&data);
        assert_eq!(
            modbus.next().await,
            Ok(RequestFrame {
                slave_id: 0x11,
                request: Request::ReadInput { address, count }
            })
        );
    }

    #[tokio::test]
    async fn fn3() {
        let bb = BBBuffer::<U2048>::new();
        let mut modbus = super::Modbus::new(&bb);

        let data = [0x11, 0x03, 0x00, 0x6B, 0x00, 0x03, 0x76, 0x87];

        let address = 0x006B;
        let count = 0x0003;

        modbus.on_data_received(&data);
        assert_eq!(
            modbus.next().await,
            Ok(RequestFrame {
                slave_id: 0x11,
                request: Request::ReadOutputRegisters { address, count }
            })
        );
    }

    #[tokio::test]
    async fn fn4() {
        let bb = BBBuffer::<U2048>::new();
        let mut modbus = super::Modbus::new(&bb);

        let data = [0x11, 0x04, 0x00, 0x08, 0x00, 0x01, 0xB2, 0x98];

        let address = 0x0008;
        let count = 0x0001;

        modbus.on_data_received(&data);
        assert_eq!(
            modbus.next().await,
            Ok(RequestFrame {
                slave_id: 0x11,
                request: Request::ReadInputRegisters { address, count }
            })
        );
    }

    #[tokio::test]
    async fn fn5_on() {
        let bb = BBBuffer::<U2048>::new();
        let mut modbus = super::Modbus::new(&bb);

        let data = [0x11, 0x05, 0x00, 0xAC, 0xFF, 0x00, 0x4E, 0x8B];

        let address = 0x00AC;

        modbus.on_data_received(&data);
        assert_eq!(
            modbus.next().await,
            Ok(RequestFrame {
                slave_id: 0x11,
                request: Request::SetCoil {
                    address,
                    status: CoilState::On
                }
            })
        );
    }

    #[tokio::test]
    async fn fn5_off() {
        let bb = BBBuffer::<U2048>::new();
        let mut modbus = super::Modbus::new(&bb);

        let data = [0x11, 0x05, 0x00, 0xAC, 0x00, 0xFF, 0x4F, 0x3B];

        let address = 0x00AC;

        modbus.on_data_received(&data);
        assert_eq!(
            modbus.next().await,
            Ok(RequestFrame {
                slave_id: 0x11,
                request: Request::SetCoil {
                    address,
                    status: CoilState::Off
                }
            })
        );
    }

    #[tokio::test]
    async fn fn6() {
        let bb = BBBuffer::<U2048>::new();
        let mut modbus = super::Modbus::new(&bb);

        let data = [0x11, 0x06, 0x00, 0x01, 0x00, 0x03, 0x9A, 0x9B];

        let address = 0x0001;
        let value = 0x0003;

        modbus.on_data_received(&data);
        assert_eq!(
            modbus.next().await,
            Ok(RequestFrame {
                slave_id: 0x11,
                request: Request::SetRegister { address, value }
            })
        );
    }

    #[tokio::test]
    async fn fn15() {
        let bb = BBBuffer::<U2048>::new();
        let mut modbus = super::Modbus::new(&bb);
        let data = [
            0x11, 0x0F, 0x00, 0x13, 0x00, 0x0A, 0x02, 0xCD, 0x01, 0xBF, 0x0B,
        ];

        let address_set = 0x0013;
        let count_set = 0x000A;

        modbus.on_data_received(&data);
        let frame = modbus.next().await;

        match frame {
            Ok(RequestFrame {
                slave_id: 0x11,
                request:
                    Request::SetCoils {
                        address,
                        count,
                        coils,
                    },
            }) => {
                assert_eq!(address_set, address);
                assert_eq!(count_set, count);

                let coils = coils.iter().collect::<Vec<_>>();
                assert_eq!(
                    coils,
                    vec![
                        CoilState::On,
                        CoilState::Off,
                        CoilState::On,
                        CoilState::On,
                        CoilState::Off,
                        CoilState::Off,
                        CoilState::On,
                        CoilState::On,
                        CoilState::On,
                        CoilState::Off
                    ]
                );
            }
            _ => panic!("Unexpected request result."),
        }
    }

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
