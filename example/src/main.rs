#![no_std]
#![no_main]

use async_embedded::task;
use cortex_m_rt::entry;
use cortex_m::asm;
use panic_rtt_core as _;
#[cfg(feature = "atomic")]
use bbqueue::atomic::BBBuffer;
#[cfg(not(feature = "atomic"))]
use bbqueue::cm_mutex::BBBuffer;
use modbus_rs;

#[entry]
fn main() -> ! {
    let bb = BBBuffer::<bbqueue::consts::U2048>::new();
    let mut modbus = modbus_rs::Modbus::new(&bb);

    let data = [0x11, 0x01, 0x00, 0x13, 0x00, 0x25, 0x0E, 0x84];

    modbus.on_data_received(&data);

    task::block_on(async {
        let _ = modbus.next().await;

        loop {
            asm::bkpt();
        }
    })
}
