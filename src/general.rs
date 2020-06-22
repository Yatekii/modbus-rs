/// Returns true if the CRC matches the data.
///
/// Expects the last two bytes of the data to be the CRC.
pub fn crc_valid(data: &[u8]) -> bool {
    crc16::State::<crc16::MODBUS>::calculate(data) == 0
}
