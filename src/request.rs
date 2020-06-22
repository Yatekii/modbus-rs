use crate::{
    consts,
    data::{CoilState, CoilStore, RegisterStore},
    error::Error,
    general,
};
use bbqueue::{ArrayLength, AutoReleaseGrantR};
use core::convert::TryInto;

pub struct RequestFrame {}

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
        registers: RegisterStore<'a, S>,
    },
}

impl<'a, S: ArrayLength<u8>> Request<'a, S> {
    pub(crate) fn parse_frame(
        rgr: AutoReleaseGrantR<'a, S>,
        frame_len: usize,
    ) -> Result<Request<'a, S>, Error> {
        let crc_valid = general::crc_valid(&rgr[..frame_len]);
        if !crc_valid {
            return Err(Error::Crc);
        }
        let _slave_address = rgr[0];
        let function_id = rgr[1];
        let data = &rgr[2..frame_len];

        let r = match function_id {
            1 => {
                let (address, count) = Self::parse_read_request(data);
                rgr.release(frame_len);
                Ok(Request::ReadCoil { address, count })
            }
            2 => {
                let (address, count) = Self::parse_read_request(data);
                rgr.release(frame_len);
                Ok(Request::ReadInput { address, count })
            }
            3 => {
                let (address, count) = Self::parse_read_request(data);
                rgr.release(frame_len);
                Ok(Request::ReadOutputRegisters { address, count })
            }
            4 => {
                let (address, count) = Self::parse_read_request(data);
                rgr.release(frame_len);
                Ok(Request::ReadInputRegisters { address, count })
            }
            5 => {
                // We use `parse_read_request` even tho it's not the classic read request.
                // But the function code 0x05 has the same format.
                let (address, status) = Self::parse_read_request(data);
                rgr.release(frame_len);
                Ok(Request::SetCoil {
                    address,
                    status: if status == 0xFF00 {
                        CoilState::On
                    } else {
                        // We assume that if the status was not 0xFF00 it was 0x0000 as expected.
                        CoilState::Off
                    },
                })
            }
            6 => {
                let (address, value) = Self::parse_read_request(data);
                rgr.release(frame_len);
                Ok(Request::SetRegister { address, value })
            }
            15 => {
                let (address, count) = Self::parse_read_request(data);
                Ok(Request::SetCoils {
                    address,
                    count,
                    coils: CoilStore::new(rgr, count as usize),
                })
            }
            16 => {
                let (address, count) = Self::parse_read_request(data);
                Ok(Request::SetRegisters {
                    address,
                    count,
                    registers: RegisterStore::new(rgr),
                })
            }
            f => Err(Error::UnknownFunction(f)),
        };

        r
    }

    fn parse_read_request<'b>(data: &'b [u8]) -> (u16, u16) {
        // The next two unwraps don't increase binary size apparently.
        let address = u16::from_be_bytes(data[0..2].try_into().unwrap_or_else(|_| panic!()));
        let count = u16::from_be_bytes(data[2..4].try_into().unwrap_or_else(|_| panic!()));

        (address, count)
    }

    /// Returns the complete length of a request dataframe including slave ID and CRC.
    /// The returned Result is always Ok except if the function code was unknown.
    /// If there was not enough databytes received yet, Ok(None) is returned.
    pub(crate) fn parse_request_len(data: &[u8]) -> Result<Option<usize>, Error> {
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
}
