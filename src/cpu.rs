//! CPU identification via the `cpuid` instruction.
//!
//! `cpuid` is how the processor describes itself. You put a "leaf" number in
//! the EAX register, run `cpuid`, and the CPU fills EAX/EBX/ECX/EDX with the
//! answer:
//!   * leaf 0        → the 12-character vendor string ("GenuineIntel", ...),
//!   * leaf 1        → family/model/stepping and a bitfield of feature flags,
//!   * leaves        → the human-readable brand name is spelled out across the
//!     0x80000002..4   three "extended" leaves, 16 bytes each.
//!
//! It's the same mechanism `cat /proc/cpuinfo` and `lscpu` are built on.

use core::arch::x86_64::__cpuid;

/// A snapshot of what `cpuid` reports about this CPU.
pub struct CpuInfo {
    pub vendor: [u8; 12],
    pub brand: [u8; 48],
    pub has_brand: bool,
    pub family: u8,
    pub model: u8,
    pub stepping: u8,
    pub features_edx: u32,
    pub features_ecx: u32,
}

/// Feature bits we care to name, as `(is_edx, bit, name)`.
pub const FEATURES: &[(bool, u32, &str)] = &[
    (true, 0, "fpu"),
    (true, 4, "tsc"),
    (true, 9, "apic"),
    (true, 23, "mmx"),
    (true, 25, "sse"),
    (true, 26, "sse2"),
    (false, 0, "sse3"),
    (false, 23, "popcnt"),
    (false, 31, "hypervisor"),
];

/// Ask the CPU to identify itself.
pub fn identify() -> CpuInfo {
    // Leaf 0: max standard leaf (eax) + vendor string (ebx, edx, ecx).
    let leaf0 = unsafe { __cpuid(0) };
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&leaf0.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&leaf0.edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&leaf0.ecx.to_le_bytes());

    // Leaf 1: version info (eax) + feature flags (edx, ecx).
    let leaf1 = unsafe { __cpuid(1) };
    let v = leaf1.eax;
    let stepping = (v & 0xF) as u8;
    let model = ((v >> 4) & 0xF) as u8;
    let family = ((v >> 8) & 0xF) as u8;

    // Extended brand string, only if the CPU advertises leaf 0x80000004.
    let ext_max = unsafe { __cpuid(0x8000_0000) }.eax;
    let has_brand = ext_max >= 0x8000_0004;
    let mut brand = [0u8; 48];
    if has_brand {
        for (i, &leaf) in [0x8000_0002u32, 0x8000_0003, 0x8000_0004].iter().enumerate() {
            let r = unsafe { __cpuid(leaf) };
            let base = i * 16;
            brand[base..base + 4].copy_from_slice(&r.eax.to_le_bytes());
            brand[base + 4..base + 8].copy_from_slice(&r.ebx.to_le_bytes());
            brand[base + 8..base + 12].copy_from_slice(&r.ecx.to_le_bytes());
            brand[base + 12..base + 16].copy_from_slice(&r.edx.to_le_bytes());
        }
    }

    CpuInfo {
        vendor,
        brand,
        has_brand,
        family,
        model,
        stepping,
        features_edx: leaf1.edx,
        features_ecx: leaf1.ecx,
    }
}

impl CpuInfo {
    /// True if the named feature bit is set.
    pub fn has(&self, is_edx: bool, bit: u32) -> bool {
        let reg = if is_edx { self.features_edx } else { self.features_ecx };
        reg & (1 << bit) != 0
    }
}
