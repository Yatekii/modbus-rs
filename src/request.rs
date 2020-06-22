use crate::{
    consts,
    data::{CoilState, CoilStore, RegisterStore},
    error::Error,
    general,
};
use bbqueue::{ArrayLength, AutoReleaseGrantR};
use core::convert::TryInto;

#[derive(Debug, PartialEq)]
pub struct RequestFrame<'a, S: ArrayLength<u8>> {
    pub(crate) slave_id: usize,
    pub(crate) request: Request<'a, S>,
}

impl<'a, S: ArrayLength<u8>> RequestFrame<'a, S> {
    /// Parses a single modbus RTU request frame.
    pub(crate) fn parse_frame(
        mut rgr: AutoReleaseGrantR<'a, S>,
        frame_len: usize,
    ) -> Result<RequestFrame<'a, S>, Error> {
        // Make sure we mark the right amount of bytes as read in our read buffer.
        rgr.to_release(frame_len);

        // Make sure the received CRC is valid.
        // If it is not valid, immediately return an error.
        let crc_valid = general::crc_valid(&rgr[..frame_len]);
        if !crc_valid {
            return Err(Error::Crc);
        }

        // Get the universal request fields.
        let slave_id = rgr[0] as usize;
        let function_id = rgr[1];

        // Get the actual data frame in the buffer, based on the frame length determined by the function id.
        let data = &rgr[2..frame_len];

        // Parse the actual requests.
        let r = match function_id {
            1 => {
                let (address, count) = Self::parse_read_request(data);
                Request::ReadCoil { address, count }
            }
            2 => {
                let (address, count) = Self::parse_read_request(data);
                Request::ReadInput { address, count }
            }
            3 => {
                let (address, count) = Self::parse_read_request(data);
                Request::ReadOutputRegisters { address, count }
            }
            4 => {
                let (address, count) = Self::parse_read_request(data);
                Request::ReadInputRegisters { address, count }
            }
            5 => {
                // We use `parse_read_request` even tho it's not the classic read request.
                // But the function code 0x05 has the same format.
                let (address, status) = Self::parse_read_request(data);
                Request::SetCoil {
                    address,
                    status: if status == 0xFF00 {
                        CoilState::On
                    } else {
                        // We assume that if the status was not 0xFF00 it was 0x0000 as expected.
                        CoilState::Off
                    },
                }
            }
            6 => {
                let (address, value) = Self::parse_read_request(data);
                Request::SetRegister { address, value }
            }
            15 => {
                let (address, count) = Self::parse_read_request(data);
                Request::SetCoils {
                    address,
                    count,
                    coils: CoilStore::new(rgr, count as usize),
                }
            }
            16 => {
                let (address, count) = Self::parse_read_request(data);
                Request::SetRegisters {
                    address,
                    count,
                    registers: RegisterStore::new(rgr),
                }
            }
            f => return Err(Error::UnknownFunction(f)),
        };

        Ok(RequestFrame {
            slave_id,
            request: r,
        })
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

    // Parses the requests for fucntion IDs 1-6.
    // Those 6 requests all share the same (u16, u16) layout which is parsed by this function.
    fn parse_read_request<'b>(data: &'b [u8]) -> (u16, u16) {
        // The next two unwraps don't increase binary size apparently.
        let address = u16::from_be_bytes(data[0..2].try_into().unwrap_or_else(|_| panic!()));
        let count = u16::from_be_bytes(data[2..4].try_into().unwrap_or_else(|_| panic!()));

        (address, count)
    }
}

/// A single modbus RTU request.
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
