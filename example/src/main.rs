#![no_std]
#![no_main]

use core::cell::RefCell;
use core::ops::DerefMut;
use cortex_m::interrupt::{free, Mutex};
use embedded_hal::timer::CountDown;
use async_embedded::task;
use cortex_m_rt::entry;
use cortex_m::asm;
use panic_rtt_target as _;
use rtt_target::{rprintln, rtt_init_print};
#[cfg(feature = "atomic")]
use bbqueue::atomic::BBBuffer;
#[cfg(not(feature = "atomic"))]
use bbqueue::cm_mutex::BBBuffer;
use modbus_rs;

use nrf52840_dk_bsp as bsp;
use bsp::hal;
use hal::target as pac;

use pac::interrupt;

/// A struct used to hold the resources we need
struct Blinky {
    /// Timer 0
    timer0: hal::Timer<pac::TIMER0, hal::timer::Periodic>,
    /// Timer 1
    timer1: hal::Timer<pac::TIMER1, hal::timer::Periodic>,
    /// LED 1
    led1: bsp::Led,
    /// LED 2
    led2: bsp::Led,
    /// button 2
    button2: bsp::Button,
    /// LED 1 state
    led1_on: bool,
}

/// Program resources
static BLINKY: Mutex<RefCell<Option<Blinky>>> = Mutex::new(RefCell::new(None));

#[entry]
fn main() -> ! {
    // Enable RTT printing
    rtt_init_print!();
    // Print a message over RTT
    rprintln!("Initialize");

    // Configure the board perephials
    if let Some(nrf52) = bsp::Board::take() {
        // Configure the timer TIMER0 to fire every second
        let mut timer0 = hal::Timer::new(nrf52.TIMER0);
        timer0.enable_interrupt();
        let mut timer0 = timer0.into_periodic();
        timer0.start(1_000_000u32);

        // Get LED 1 and turn it off
        let mut led1 = nrf52.leds.led_1;
        led1.disable();

        // Get LED 2 and turn it off
        let mut led2 = nrf52.leds.led_2;
        led2.disable();

        // Clear timer events
        let _ = timer0.wait();

        // Enable interrupts
        unsafe {
            pac::NVIC::unmask(pac::Interrupt::TIMER0);
            pac::NVIC::unmask(pac::Interrupt::TIMER1);
        }
        // Create Blinky struct
        let blinky = Blinky {
            timer0,
            led1,
            led2,
            button2: nrf52.buttons.button_2,
            led1_on: false,
        };
        // Store the Blinky struct in the static variable
        free(|cs| {
            BLINKY.borrow(cs).replace(Some(blinky));
        });
    }

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

#[interrupt]
fn TIMER0() {
    // Lock the Mutex to borrow the resources
    free(|cs| {
        if let Some(ref mut blinky) = BLINKY.borrow(cs).borrow_mut().deref_mut() {
            // Wait and reset the timer event
            let _ = blinky.timer0.wait();
            // Check the state variable
            if blinky.led1_on {
                // Enable LED 1
                blinky.led1.enable();
            } else {
                // Disable LED 1
                blinky.led1.disable();
            }
            // Update the state variable
            blinky.led1_on = !blinky.led1_on;
        }
    });
    // Print a message over RTT
    rprintln!("TIMER0");
}

#[interrupt]
fn TIMER1() {
    // Lock the Mutex to borrow the resources
    free(|cs| {
        if let Some(ref mut blinky) = BLINKY.borrow(cs).borrow_mut().deref_mut() {
            // Wait and reset the timer event
            let _ = blinky.timer1.wait();
            // Check if the button is pressed
            if blinky.button2.is_pressed() {
                // Enable LED 2
                blinky.led2.enable();
            } else {
                // Disable LED 2
                blinky.led2.disable();
            }
        }
    });
}