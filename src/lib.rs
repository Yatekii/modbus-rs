use nom::{bytes::streaming::take, *};

pub struct Frame {}
// pub struct Request {}
// pub struct Response {}

pub enum Status {
    On = 0xFF00,
    Off = 0x0000,
}

pub enum Request {
    // FnCode = 1
    ReadCoils { address: u16, count: u16 },
    // FnCode = 2
    ReadDiscreteInputs { address: u16, count: u16 },
    // FnCode = 3
    ReadMultipleHoldingRegisters { address: u16, count: u16 },
    // FnCode = 4
    ReadInputRegisters { address: u16, count: u16 },
    // FnCode = 5
    WriteSingleCoil { address: u16, value: Status },
    // FnCode = 6
    WriteSingleHoldingRegister { address: u16, value: u16 },
    // FnCode = 15
    WriteMultipleCoils { address: u16 },
    // FnCode = 16
    WriteMultipleHoldingRegisters = 16,
}

fn parse_modbus_frame(input: &[u8]) -> nom::IResult<&[u8], Frame, ()> {
    let (input, address) = take(1)(input)?;
    let (input, function) = take(1)(input)?;
    // Check function
    // let (input, function) = take(1)(input)?;
    let (input, crc) = take(2)(input)?;
    // Check CRC
}

// fn parse_modbus_request(start: &[u8]) -> nom::IResult<&[u8], Request, ()> {}

// fn parse_modbus_response(start: &[u8]) -> nom::IResult<&[u8], Response, ()> {}

#[cfg(test)]
mod tests {
    #[test]
    fn test() {
        println!("Hello, world!");
    }
}
