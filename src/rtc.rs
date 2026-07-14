//! Reading the real-time clock (RTC).
//!
//! Every PC has a battery-backed **CMOS** chip that keeps the date and time
//! running even while the machine is off. We talk to it through two I/O ports:
//! write a register number to `0x70`, then read that register's value from
//! `0x71`. The clock fields (seconds, minutes, ..., year) live in fixed
//! registers.
//!
//! Two quirks make this fiddlier than it looks:
//!   1. The values are usually **BCD** — each byte stores two decimal digits in
//!      its two nibbles (0x59 means 59, not 89). Status Register B says which.
//!   2. The clock **ticks while you read it**. If you read the seconds field
//!      right as it rolls over, you can catch it mid-update and get garbage. So
//!      we wait for the "update in progress" flag to clear, and re-read until
//!      two consecutive reads agree.

use x86_64::instructions::port::Port;

const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

/// A moment read from the real-time clock (always UTC on a typical PC).
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Read one CMOS register (select on port 0x70, read from 0x71).
unsafe fn read_reg(reg: u8) -> u8 {
    let mut addr: Port<u8> = Port::new(CMOS_ADDR);
    let mut data: Port<u8> = Port::new(CMOS_DATA);
    addr.write(reg);
    data.read()
}

/// Status Register A bit 7 is set while the clock is mid-update.
unsafe fn update_in_progress() -> bool {
    read_reg(0x0A) & 0x80 != 0
}

/// The six clock fields, read in one pass.
#[derive(PartialEq, Clone, Copy)]
struct Raw {
    sec: u8,
    min: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8,
}

unsafe fn read_raw() -> Raw {
    Raw {
        sec: read_reg(0x00),
        min: read_reg(0x02),
        hour: read_reg(0x04),
        day: read_reg(0x07),
        month: read_reg(0x08),
        year: read_reg(0x09),
    }
}

/// Read the current date/time, decoding BCD and re-reading until stable.
pub fn now() -> DateTime {
    unsafe {
        // Take reads until two in a row agree (never mid-tick).
        while update_in_progress() {}
        let mut prev = read_raw();
        loop {
            while update_in_progress() {}
            let cur = read_raw();
            if cur == prev {
                break;
            }
            prev = cur;
        }
        let raw = prev;

        // Status Register B: bit 2 set => values are binary; else BCD.
        //                    bit 1 set => 24-hour; else 12-hour with a PM flag.
        let reg_b = read_reg(0x0B);
        let is_binary = reg_b & 0x04 != 0;
        let is_24h = reg_b & 0x02 != 0;

        // Convert a raw byte to a number, decoding BCD if needed.
        let dec = |v: u8| if is_binary { v } else { (v & 0x0F) + ((v >> 4) * 10) };

        // The high bit of the hour byte is the PM flag in 12-hour mode.
        let mut hour = dec(raw.hour & 0x7F);
        if !is_24h && (raw.hour & 0x80 != 0) {
            hour = (hour % 12) + 12; // PM
        }

        DateTime {
            year: 2000 + dec(raw.year) as u16, // 2-digit year; we live in the 2000s
            month: dec(raw.month),
            day: dec(raw.day),
            hour,
            minute: dec(raw.min),
            second: dec(raw.sec),
        }
    }
}
