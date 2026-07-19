//! Finding the ACPI tables: the RSDP, then the RSDT/XSDT table list.
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
//! Once the RSDP is in hand, `list_tables` walks the next layer: the **RSDT**
//! (or, on ACPI 2.0+, the **XSDT**) — a table that is itself just an array of
//! pointers to every other ACPI table on the machine (the FADT, the MADT/APIC
//! table, HPET, ...). We don't decode any of *those* tables' contents yet; we
//! just find and validate each one, which is the necessary next step before
//! any of them can be understood.

use alloc::vec::Vec;

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

/// Every ACPI table's data starts with the same 36-byte **SDT header**:
/// a 4-byte signature, a 4-byte length (covering the *whole* table, header
/// included), then revision/checksum/OEM fields we don't need yet.
const SDT_HEADER_LEN: usize = 36;

/// One table found via the RSDT/XSDT: its signature, size, where it lives, and
/// whether its checksum actually validated.
pub struct TableInfo {
    pub signature: [u8; 4],
    pub length: u32,
    pub physical_address: u64,
    pub checksum_ok: bool,
}

/// Read a table's SDT header and check its signature + full-table checksum.
/// Returns `None` only if the memory can't even be read as a header-shaped
/// blob; a failed checksum is still reported (as `checksum_ok: false`) rather
/// than hidden, since "found it but it's corrupt" is useful information too.
fn describe_table(phys: u64) -> Option<TableInfo> {
    let header = unsafe { read_bytes(phys, SDT_HEADER_LEN) };
    let mut signature = [0u8; 4];
    signature.copy_from_slice(&header[0..4]);
    let length = u32::from_le_bytes(header[4..8].try_into().ok()?);

    // The checksum covers the ENTIRE table (not just the header), so re-read
    // the full `length` bytes to verify it — the header alone isn't enough.
    let whole = unsafe { read_bytes(phys, length as usize) };
    let valid = checksum_ok(whole);

    Some(TableInfo {
        signature,
        length,
        physical_address: phys,
        checksum_ok: valid,
    })
}

/// Walk the RSDT (or XSDT, if this machine is ACPI 2.0+ and provides one) and
/// describe every table it points to. This is the "front door" from
/// `find_rsdp` leading to the rest of the house: the RSDT/XSDT is itself just
/// an array of pointers to the real tables (FADT, MADT/APIC, HPET, ...).
pub fn list_tables(rsdp: &RsdpInfo) -> Vec<TableInfo> {
    let mut tables = Vec::new();

    // Prefer the XSDT (64-bit pointers) when ACPI 2.0+ actually supplies one;
    // otherwise fall back to the RSDT (32-bit pointers) every version has.
    let use_xsdt = rsdp.revision >= 2 && rsdp.xsdt_address != 0;
    let root_addr = if use_xsdt {
        rsdp.xsdt_address
    } else {
        rsdp.rsdt_address as u64
    };
    let pointer_size: usize = if use_xsdt { 8 } else { 4 };

    // Describe the root table itself first (its signature should read "XSDT"
    // or "RSDT" — a good sanity check that we're pointed at the right place).
    let Some(root) = describe_table(root_addr) else {
        return tables;
    };
    let entry_count = (root.length as usize).saturating_sub(SDT_HEADER_LEN) / pointer_size;
    tables.push(root);

    // The pointer array follows immediately after the root table's header.
    let entries_start = root_addr + SDT_HEADER_LEN as u64;
    for i in 0..entry_count {
        let entry_addr = entries_start + (i * pointer_size) as u64;
        let pointer = if use_xsdt {
            let bytes = unsafe { read_bytes(entry_addr, 8) };
            u64::from_le_bytes(bytes.try_into().unwrap_or([0; 8]))
        } else {
            let bytes = unsafe { read_bytes(entry_addr, 4) };
            u32::from_le_bytes(bytes.try_into().unwrap_or([0; 4])) as u64
        };
        if let Some(table) = describe_table(pointer) {
            tables.push(table);
        }
    }

    tables
}
