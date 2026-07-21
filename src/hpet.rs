//! Reading the HPET (High Precision Event Timer) via memory-mapped I/O.
//!
//! Every hardware access so far — the PIT, the CMOS RTC, the keyboard, PCI
//! configuration space — has gone through **port I/O**: special `in`/`out`
//! instructions talking to a numbered port. The HPET is different: it's
//! **memory-mapped**. Its registers live at an ordinary physical address, and
//! you read or write them with plain memory loads and stores, no special
//! instruction at all. The only "trick" is finding that address in the first
//! place — which is exactly what the HPET's ACPI table (found in Stage 18,
//! decoded here) is for.
//!
//! We don't use the HPET as the kernel's clock (the PIT from Stage 2 still
//! drives that) — this is a read-only look at a second, independent hardware
//! timer, and proof the kernel can do memory-mapped device access at all.

use crate::acpi::TableInfo;
use crate::memory::phys_to_virt;
use x86_64::PhysAddr;

/// Matches `acpi::SDT_HEADER_LEN` — every ACPI table shares this 36-byte
/// header before its type-specific fields begin.
const SDT_HEADER_LEN: u64 = 36;

/// HPET register offsets, from the HPET spec, relative to its base address.
const REG_CAPABILITIES: u64 = 0x000;
const REG_CONFIG: u64 = 0x010;
const REG_MAIN_COUNTER: u64 = 0xF0;

/// Read one 64-bit **device register** at physical address `phys` via an
/// ordinary volatile load through the offset-mapped view of physical memory.
/// `read_volatile` matters here for the same reason it did for the VGA
/// buffer: the compiler can't see that this "memory" is actually a live
/// device register, so without it, it might optimize away a "redundant"
/// second read — which is exactly the read we need, to prove the counter is
/// moving. This requires 8-byte *alignment*, which is safe to assume here:
/// hardware convention places an MMIO device's base address on at least a
/// 4 KiB boundary, so `base + small offset` is always 8-byte aligned too.
unsafe fn read_mmio64(phys: u64) -> u64 {
    let virt = phys_to_virt(PhysAddr::new(phys));
    core::ptr::read_volatile(virt.as_ptr::<u64>())
}

unsafe fn write_mmio64(phys: u64, value: u64) {
    let virt = phys_to_virt(PhysAddr::new(phys));
    core::ptr::write_volatile(virt.as_mut_ptr::<u64>(), value);
}

/// Read a **possibly-unaligned** 64-bit value at a physical address — for
/// fields packed *inside* an ACPI table, which (unlike device MMIO
/// registers) the spec does not promise to be 8-byte aligned. A plain aligned
/// `read_volatile::<u64>` would be undefined behavior here; `read_unaligned`
/// is the correct tool, and volatility doesn't matter since static BIOS table
/// data can't change out from under us the way a live register can.
unsafe fn read_unaligned_phys64(phys: u64) -> u64 {
    let virt = phys_to_virt(PhysAddr::new(phys));
    core::ptr::read_unaligned(virt.as_ptr::<u64>())
}

/// What we learn from the HPET.
pub struct HpetInfo {
    pub base_address: u64,
    pub num_timers: u8,
    pub counter_is_64bit: bool,
    /// How long one tick of the main counter represents, in femtoseconds
    /// (10^-15 s) — the HPET spec's native unit, chosen so the period is
    /// always a whole number regardless of the counter's actual frequency.
    pub period_femtoseconds: u32,
    pub counter_before: u64,
    pub counter_after: u64,
    /// True if we had to flip the HPET's own enable bit ourselves because we
    /// found it switched off — some firmware leaves it disabled at boot,
    /// trusting the OS to turn it on when it actually wants to use it.
    pub we_enabled_it: bool,
}

impl HpetInfo {
    /// Nanoseconds elapsed between the two counter reads, computed from the
    /// hardware's own reported tick period rather than assumed.
    pub fn elapsed_ns(&self) -> u64 {
        let ticks = self.counter_after.wrapping_sub(self.counter_before);
        // femtoseconds -> nanoseconds is a division by 1,000,000; do the
        // multiply first, in a wider type, so we don't lose precision.
        ((ticks as u128 * self.period_femtoseconds as u128) / 1_000_000) as u64
    }
}

/// Locate, read, and (if needed) start the HPET named by an ACPI `"HPET"`
/// table, then take two counter readings to show it actually advancing.
pub fn read(table: &TableInfo) -> Option<HpetInfo> {
    if &table.signature != b"HPET" {
        return None;
    }

    // The HPET Description Table's layout (per the ACPI spec): after the
    // standard 36-byte header comes a 4-byte hardware ID, then a 12-byte
    // Generic Address Structure whose last 8 bytes are the HPET's base
    // physical address — this is what turns "we found an ACPI table" into
    // "we know where the actual device registers live".
    let addr_field = table.physical_address + SDT_HEADER_LEN + 4 + 4;
    let base_address = unsafe { read_unaligned_phys64(addr_field) };

    let caps = unsafe { read_mmio64(base_address + REG_CAPABILITIES) };
    let counter_is_64bit = (caps >> 13) & 1 != 0;
    let num_timers = (((caps >> 8) & 0x1F) + 1) as u8;
    let period_femtoseconds = (caps >> 32) as u32;

    // The main counter only advances once the ENABLE_CNF bit (bit 0 of the
    // configuration register) is set. Some firmware leaves it off until an OS
    // asks for it — so check, and turn it on ourselves if it's not running.
    let config = unsafe { read_mmio64(base_address + REG_CONFIG) };
    let we_enabled_it = config & 1 == 0;
    if we_enabled_it {
        unsafe { write_mmio64(base_address + REG_CONFIG, config | 1) };
    }

    let counter_before = unsafe { read_mmio64(base_address + REG_MAIN_COUNTER) };
    for _ in 0..200_000 {
        core::hint::spin_loop(); // burn a little real time between readings
    }
    let counter_after = unsafe { read_mmio64(base_address + REG_MAIN_COUNTER) };

    Some(HpetInfo {
        base_address,
        num_timers,
        counter_is_64bit,
        period_femtoseconds,
        counter_before,
        counter_after,
        we_enabled_it,
    })
}
