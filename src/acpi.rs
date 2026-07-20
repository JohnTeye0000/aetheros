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
//! table, HPET, ...). We find and validate each one, but decode the *contents*
//! of only one so far: `parse_madt` reads the **MADT** to answer the question
//! a future multi-core kernel needs answered first — how many CPU cores does
//! this machine actually have?

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

/// What the MADT (the table with signature `"APIC"`) says about the machine's
/// interrupt-controller topology — most usefully, how many CPU cores exist.
pub struct MadtInfo {
    pub local_apic_address: u32,
    /// One entry per **enabled** processor local APIC — in practice, one per
    /// usable CPU core. This is the number a future multi-core kernel would
    /// read to know how many cores it has to bring up.
    pub cpu_apic_ids: Vec<u8>,
    /// One entry per I/O APIC: `(id, address, global_system_interrupt_base)`.
    pub io_apics: Vec<(u8, u32, u32)>,
}

/// Parse the MADT's variable-length entry list.
///
/// After the standard 36-byte SDT header, the MADT has an 8-byte fixed part
/// (the local APIC's physical address, then a flags word), followed by a
/// stream of `(type: u8, length: u8, ...type-specific data...)` entries
/// packed back-to-back until the table ends. We only decode the two entry
/// types we care about right now (0 = processor local APIC, 1 = I/O APIC);
/// everything else we skip using its length, which is exactly what a real
/// parser does with entry types it doesn't (yet) understand — the length
/// byte is what makes the format forward-compatible.
pub fn parse_madt(table: &TableInfo) -> Option<MadtInfo> {
    if &table.signature != b"APIC" {
        return None;
    }

    let data = unsafe { read_bytes(table.physical_address, table.length as usize) };
    if data.len() < SDT_HEADER_LEN + 8 {
        return None;
    }

    let local_apic_address = u32::from_le_bytes(
        data[SDT_HEADER_LEN..SDT_HEADER_LEN + 4].try_into().ok()?,
    );

    let mut cpu_apic_ids = Vec::new();
    let mut io_apics = Vec::new();

    let mut i = SDT_HEADER_LEN + 8; // past the header and the two fixed fields
    while i + 2 <= data.len() {
        let entry_type = data[i];
        let entry_len = data[i + 1] as usize;
        if entry_len < 2 || i + entry_len > data.len() {
            break; // a malformed length would read past the table; stop rather than guess
        }

        match entry_type {
            // Processor Local APIC: ACPI Processor ID, APIC ID, then a 4-byte
            // flags word whose bit 0 says whether this CPU is actually usable.
            0 if entry_len >= 8 => {
                let apic_id = data[i + 3];
                let flags = u32::from_le_bytes(data[i + 4..i + 8].try_into().ok()?);
                if flags & 1 != 0 {
                    cpu_apic_ids.push(apic_id);
                }
            }
            // I/O APIC: its own ID, physical address, and the first global
            // system interrupt (GSI) number it's responsible for.
            1 if entry_len >= 12 => {
                let id = data[i + 2];
                let address = u32::from_le_bytes(data[i + 4..i + 8].try_into().ok()?);
                let gsi_base = u32::from_le_bytes(data[i + 8..i + 12].try_into().ok()?);
                io_apics.push((id, address, gsi_base));
            }
            _ => {} // interrupt source overrides, NMIs, etc. — not needed yet
        }

        i += entry_len;
    }

    Some(MadtInfo {
        local_apic_address,
        cpu_apic_ids,
        io_apics,
    })
}
