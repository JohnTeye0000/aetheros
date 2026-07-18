# Devlog 017 — Finding the ACPI tables (the RSDP)

**Milestone:** the kernel can locate the ACPI **RSDP** — the one structure everything else in ACPI hangs off of — and validate that it's genuine before trusting anything in it. `acpi` finds it, checksums it, and reports the version and table addresses.

## What ACPI is for, and where it starts

The firmware knows things the kernel doesn't: how to power the machine off, what timers and buses exist, how many CPUs there are. **ACPI** is the standardized way it hands that knowledge over — a set of tables in memory, each with its own structure, describing one aspect of the machine. But there's no register that says "the tables start here." Instead there's exactly one signpost, the **Root System Description Pointer (RSDP)**, and finding it is a search, not a lookup.

## Searching for a needle you can recognize

The RSDP's first 8 bytes are always the ASCII string `"RSD PTR "` (with a trailing space) — a fixed signature the spec guarantees. The firmware places the structure at a 16-byte-aligned address in one of two places:

1. the first 1 KiB of the **Extended BIOS Data Area** (EBDA) — whose location is itself found by reading a 16-bit segment value the BIOS leaves at the fixed physical address `0x40E`,
2. or the read-only BIOS region `0xE0000`–`0xFFFFF`.

So `find_rsdp` checks the EBDA first, then falls back to the BIOS region — scanning 16 bytes at a time and comparing against the signature, exactly like grepping for a string, except over physical memory instead of a file.

## Reading physical memory by address

This is the first time the kernel needed to read memory at a **specific physical address** rather than through the normal virtual-address path. Stage 4's paging work already solved the hard part: the bootloader maps all of physical RAM at a known offset, so physical address `p` is readable at `offset + p`. That offset was previously a local variable inside `kernel_main`; this stage stashes it in an atomic (`memory::phys_to_virt`) so any later part of the kernel — `acpi`, and whatever wants to read a fixed physical address next — can translate physical to virtual without threading the offset through every function signature.

## Never trust a signature match alone

Finding the 8 magic bytes isn't proof — memory is full of coincidental byte patterns. ACPI's real safeguard is a **checksum**: sum every byte of the structure, and a genuine one always sums to zero (mod 256), because the firmware chose the checksum byte to make that true. So `find_rsdp` doesn't return anything until `checksum_ok` passes — first over the 20-byte ACPI 1.0 structure, and (for ACPI 2.0+, which adds a 64-bit XSDT pointer) again over the full 36 bytes. Any match that fails the checksum is treated as a coincidence, not a table.

## Proof

The demo output is real, and shows the checksum genuinely doing its job — it's not printed unless verification passed:

```
RSDP found at physical 0xf52e0
OEM: BOCHS   ACPI version: 1.0 (RSDT only)
RSDT: 0x7fe2328
```

`0xf52e0` falls right in the documented BIOS scan range, and `OEM: BOCHS` is a real fingerprint — QEMU's firmware (SeaBIOS) descends from the Bochs BIOS lineage and leaves that OEM ID in its ACPI tables.

## What this sets up

We don't parse the RSDT/XSDT yet — that's a table of *pointers to more tables* (APIC/MADT for multi-core info, HPET for a better timer, FADT for power management), each needing its own decoder. But every one of them starts by finding this pointer first. Today's milestone is the front door; the rooms behind it are future work.

## Next

The two large remaining milestones — preemptive scheduling and a real disk driver — stay queued for slower, multi-session work.
