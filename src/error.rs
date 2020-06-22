#[derive(Debug, PartialEq)]
pub enum Error {
    Crc,
    UnknownFunction(u8),
}
