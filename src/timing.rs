//! Fine-grained time: the CPU's timestamp counter (TSC), calibrated against
//! the PIT.
//!
//! The timer interrupt gives the kernel a clock with 1 ms resolution — fine for
//! deciding when to switch threads, far too coarse for *measuring* scheduling
//! behaviour (a wake-up delay of 40 µs and one of 900 µs would both read as
//! "0 ticks"). `rdtsc` reads a 64-bit counter that increments at a fixed rate
//! (billions of times a second on real hardware). We don't know that rate up
//! front, so at boot we count how many TSC cycles pass during a known number
//! of PIT ticks and derive cycles-per-second from that.
//!
//! Caveat for anyone reading the numbers: under QEMU's software emulation the
//! TSC is derived from QEMU's virtual clock, so it measures *emulated* time.
//! Run with `-icount` and that virtual clock advances per executed
//! instruction, making runs reproducible; without it, host load leaks in.

use core::sync::atomic::{AtomicU64, Ordering};

static TSC_HZ: AtomicU64 = AtomicU64::new(0);
static BOOT_TSC: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
pub fn now() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Measure the TSC rate over `ms` milliseconds of PIT ticks. Interrupts must be
/// enabled and the PIT programmed (see `interrupts::init_hardware`).
pub fn calibrate(ms: u64) {
    let ticks_per_ms = crate::interrupts::HZ / 1000;
    // Align to a tick edge so we measure whole ticks.
    let t0 = crate::interrupts::ticks();
    while crate::interrupts::ticks() == t0 {
        x86_64::instructions::hlt();
    }
    let start_tick = crate::interrupts::ticks();
    let start = now();
    let target = start_tick + ms * ticks_per_ms;
    while crate::interrupts::ticks() < target {
        x86_64::instructions::hlt();
    }
    let cycles = now() - start;
    TSC_HZ.store(cycles * 1000 / ms, Ordering::SeqCst);
    BOOT_TSC.store(start, Ordering::SeqCst);
}

pub fn tsc_hz() -> u64 {
    TSC_HZ.load(Ordering::Relaxed)
}

/// Convert a cycle count to microseconds.
pub fn cycles_to_us(cycles: u64) -> u64 {
    let hz = tsc_hz();
    if hz == 0 {
        return 0;
    }
    ((cycles as u128 * 1_000_000) / hz as u128) as u64
}

/// Microseconds since calibration.
pub fn uptime_us() -> u64 {
    cycles_to_us(now().saturating_sub(BOOT_TSC.load(Ordering::Relaxed)))
}
