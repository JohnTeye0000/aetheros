//! Finding the ACPI tables (the RSDP).
//!
//! ACPI (Advanced Configuration and Power Interface) is how firmware hands the
//! OS a structured description of the machine — how to power it off, what
//! timers exist, which CPUs are present. Everything ACPI starts from one
//! signpost: the **RSDP** (Root System Description Pointer), a small structure
//! the firmware leaves sitting in memory for the OS to find. There's no
//! register or port that hands it to you — you *search* for it, by looking for
//! its 8-byte signature at 16-byte-aligned addresses in two specific places the
//! spec guarantees to check.
//!
//! We don't parse anything ACPI points *at* yet (the RSDT/XSDT and the tables
//! they list) — finding the RSDP itself, correctly, is the whole milestone.
//! Everything else ACPI can do starts by knowing where this is.

use crate::memory::phys_to_virt;
use x86_64::PhysAddr;

const SIGNATURE: &[u8; 8] = b"RSD PTR ";

/// What we learn from a valid RSDP.
pub struct RsdpInfo {
    pub physical_address: u64,
    pub oem_id: [u8; 6],
    pub revision: u8,
    pub rsdt_address: u32,
    /// Only meaningful when `revision >= 2` (ACPI 2.0+).
    pub xsdt_address: u64,
}

/// Read `len` bytes at physical address `phys` via the identity-plus-offset
/// mapping — safe as long as that physical range is genuinely mapped, which it
/// is for anything in the first megabyte on a PC.
unsafe fn read_bytes(phys: u64, len: usize) -> &'static [u8] {
    let virt = phys_to_virt(PhysAddr::new(phys));
    core::slice::from_raw_parts(virt.as_ptr::<u8>(), len)
}

/// Sum every byte in `data`; a valid ACPI structure always sums to zero (mod
/// 256) over its own checksum field, so success looks like `sum == 0`.
fn checksum_ok(data: &[u8]) -> bool {
    data.iter().fold(0u8, |acc, &b| acc.wrapping_add(b)) == 0
}

/// Search one physical range, 16 bytes at a time, for the RSDP signature.
fn scan_range(start: u64, end: u64) -> Option<u64> {
    let mut addr = start;
    while addr < end {
        let sig = unsafe { read_bytes(addr, 8) };
        if sig == SIGNATURE {
            return Some(addr);
        }
        addr += 16; // the spec guarantees 16-byte alignment
    }
    None
}

/// Locate and validate the RSDP. The spec says to check two places: the first
/// KiB of the Extended BIOS Data Area, then the BIOS read-only region
/// `0xE0000..0xFFFFF`. We check both, EBDA first (that's the documented order).
pub fn find_rsdp() -> Option<RsdpInfo> {
    // The EBDA's segment (paragraph, i.e. x16) address is a BIOS-provided
    // 16-bit word living at the fixed physical address 0x40E.
    let ebda_segment = u16::from_le_bytes(unsafe { read_bytes(0x40E, 2) }.try_into().ok()?);
    let ebda_start = (ebda_segment as u64) << 4;

    let found = if ebda_start != 0 {
        scan_range(ebda_start, ebda_start + 1024).or_else(|| scan_range(0xE0000, 0xFFFFF))
    } else {
        scan_range(0xE0000, 0xFFFFF)
    }?;

    // Validate the first 20 bytes (the ACPI 1.0 structure) before trusting
    // anything in it — a signature match alone isn't proof; the checksum is.
    let head = unsafe { read_bytes(found, 20) };
    if !checksum_ok(head) {
        return None;
    }

    let mut oem_id = [0u8; 6];
    oem_id.copy_from_slice(&head[9..15]);
    let revision = head[15];
    let rsdt_address = u32::from_le_bytes(head[16..20].try_into().ok()?);

    // ACPI 2.0+ extends the structure to 36 bytes with a 64-bit XSDT pointer
    // and its own checksum covering the whole thing.
    let xsdt_address = if revision >= 2 {
        let full = unsafe { read_bytes(found, 36) };
        if !checksum_ok(full) {
            return None;
        }
        u64::from_le_bytes(full[24..32].try_into().ok()?)
    } else {
        0
    };

    Some(RsdpInfo {
        physical_address: found,
        oem_id,
        revision,
        rsdt_address,
        xsdt_address,
    })
}
