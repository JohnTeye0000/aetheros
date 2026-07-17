//! PCI bus enumeration.
//!
//! Every device on the PCI bus — the host bridge, disk controllers, the
//! graphics card — exposes a small "configuration space" the CPU can read over
//! two I/O ports, no driver needed: write a bus/device/function/offset to the
//! **address port** (`0xCF8`), then read the answer from the **data port**
//! (`0xCFC`). This is how an OS discovers what hardware exists *before* it
//! knows anything else about the machine — the same mechanism `lspci` uses.
//!
//! We don't talk to any of these devices yet (that's a driver, a much bigger
//! job) — we just enumerate what's there and identify it. That's still real:
//! it's the first step of every PCI driver, and the identifiers it returns are
//! genuine hardware IDs, not something the kernel invents.

use alloc::vec::Vec;
use x86_64::instructions::port::Port;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

/// One PCI function found on the bus.
pub struct Device {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
}

/// Read one 32-bit configuration-space register.
///
/// The address word's layout is fixed by the PCI spec: bit 31 enables the
/// access, bits 16-23/11-15/8-10 pick the bus/device/function, and the low
/// byte is the register offset (must be 4-byte aligned — PCI config space is
/// read a dword at a time, even for byte-sized fields).
fn config_read32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address: u32 = (1 << 31)
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);

    unsafe {
        let mut addr_port: Port<u32> = Port::new(CONFIG_ADDRESS);
        let mut data_port: Port<u32> = Port::new(CONFIG_DATA);
        addr_port.write(address);
        data_port.read()
    }
}

/// Probe one bus/device/function slot. Returns `None` if nothing is there —
/// an absent slot reads back all-ones (vendor ID `0xFFFF`), since the bus has
/// no device to pull the data lines low.
fn probe(bus: u8, device: u8, function: u8) -> Option<Device> {
    let word0 = config_read32(bus, device, function, 0x00);
    let vendor_id = (word0 & 0xFFFF) as u16;
    if vendor_id == 0xFFFF {
        return None;
    }
    let device_id = (word0 >> 16) as u16;

    // Offset 0x08: revision (byte 0), then prog-if, subclass, class (bytes 1-3).
    let word2 = config_read32(bus, device, function, 0x08);
    let class = (word2 >> 24) as u8;
    let subclass = (word2 >> 16) as u8;

    Some(Device {
        bus,
        device,
        function,
        vendor_id,
        device_id,
        class,
        subclass,
    })
}

/// Whether a device is multi-function (bit 7 of the header-type byte). If not,
/// there's no point probing functions 1-7 — the slot only has function 0.
fn is_multifunction(bus: u8, device: u8) -> bool {
    let word3 = config_read32(bus, device, 0, 0x0C);
    let header_type = (word3 >> 16) as u8;
    header_type & 0x80 != 0
}

/// Walk every device/function on **bus 0** and collect what answers.
///
/// A thorough scanner follows PCI-to-PCI bridges to reach other buses, but
/// that needs its own bookkeeping (a bridge's config space names the bus
/// numbers behind it) and is a driver-shaped problem in its own right. On the
/// PC platform (and every stock QEMU machine) essentially everything of
/// interest — the host bridge, ISA/IDE/ACPI bridges, the display — sits on
/// bus 0, so a single-bus scan already finds the real hardware. Following
/// bridges to reach deeper buses is a natural next step, not a correctness
/// gap in what this reports.
pub fn scan() -> Vec<Device> {
    let mut found = Vec::new();
    let bus = 0u8;
    for device in 0..32u8 {
        if let Some(d) = probe(bus, device, 0) {
            let multi = is_multifunction(bus, device);
            found.push(d);
            if multi {
                for function in 1..8u8 {
                    if let Some(d) = probe(bus, device, function) {
                        found.push(d);
                    }
                }
            }
        }
    }
    found
}

/// A short human name for a few well-known (vendor, device) pairs — the ones a
/// stock QEMU machine actually exposes. Anything else just prints its IDs;
/// there's no room in a hobby kernel for the real world's thousands of
/// vendor/device tables.
pub fn known_name(vendor: u16, device: u16) -> Option<&'static str> {
    match (vendor, device) {
        (0x8086, 0x1237) => Some("Intel 82441FX host bridge"),
        (0x8086, 0x7000) => Some("Intel PIIX3 ISA bridge"),
        (0x8086, 0x7010) => Some("Intel PIIX3 IDE controller"),
        (0x8086, 0x7020) => Some("Intel PIIX3 USB controller"),
        (0x8086, 0x7113) => Some("Intel PIIX4 ACPI"),
        (0x8086, 0x100e) => Some("Intel e1000 network card"),
        (0x1234, 0x1111) => Some("QEMU standard VGA"),
        (0x1af4, _) => Some("VirtIO device"),
        _ => None,
    }
}

/// A short name for a class code, per the PCI class-code spec.
pub fn class_name(class: u8) -> &'static str {
    match class {
        0x00 => "unclassified",
        0x01 => "mass storage",
        0x02 => "network",
        0x03 => "display",
        0x04 => "multimedia",
        0x05 => "memory",
        0x06 => "bridge",
        0x07 => "comms",
        0x0C => "serial bus",
        _ => "other",
    }
}
