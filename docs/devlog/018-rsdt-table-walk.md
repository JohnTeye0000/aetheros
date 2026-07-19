# Devlog 018 — Walking the RSDT (listing every ACPI table)

**Milestone:** `acpi` now walks past the RSDP to the tables it points at — listing every ACPI table on the machine by name, size, and address, with each one checksum-verified. Yesterday's devlog called the RSDP "the front door"; this is opening it.

## The RSDT/XSDT is just an array of pointers

Every ACPI table shares the same 36-byte **SDT header**: a 4-byte signature, a 4-byte length covering the whole table (header included), then some fields we don't need yet. The RSDT and XSDT are themselves ordinary tables with that same header — their signature is literally `"RSDT"` or `"XSDT"` — except their *payload*, right after the header, is nothing but a flat array of pointers to every other table on the system. 32-bit pointers for the RSDT (every ACPI version has one), 64-bit for the XSDT (only ACPI 2.0+, and only if the firmware actually populated it).

So reading the table list is arithmetic once you have the root table's header: `entry_count = (length - 36) / pointer_size`, then walk that many pointer-sized slots starting right after the header.

## Reusing "trust but verify"

Rather than write new validation logic, `describe_table` reuses the exact same `checksum_ok` helper from finding the RSDP — because the rule is identical: **every** ACPI structure, not just the RSDP, is only trustworthy if summing all its bytes comes out to zero. The one wrinkle is *which* bytes: the checksum covers the table's *entire* length, not just the 36-byte header, so verifying a table means reading it twice — once for the header (to learn how long it is), once more at its full length (to actually check it).

I also chose to report a bad checksum rather than silently drop it (`checksum_ok: false` instead of `None`). "This signature exists but failed verification" is a more honest answer than making a corrupt table disappear as if it were never there — the same instinct as reporting a page fault instead of pretending the CPU handled it fine.

## What came back

QEMU's SeaBIOS hands over five real tables, and every one of them checksums clean:

```
RSDT     52 bytes  @ 0x07fe2328  [ok]
FACP    116 bytes  @ 0x07fe21dc  [ok]
APIC    120 bytes  @ 0x07fe2250  [ok]
HPET     56 bytes  @ 0x07fe22c8  [ok]
WAET     40 bytes  @ 0x07fe2300  [ok]
```

These aren't arbitrary names — each one is a real, documented ACPI table with a job: `FACP` (the **FADT**) describes fixed power-management hardware, `APIC` (the **MADT**) lists every CPU and interrupt controller in the machine, `HPET` describes a higher-precision timer than the PIT we've been using since Stage 2, and `WAET` is a Windows-specific hint table. We don't parse any of their *contents* yet — but the MADT, in particular, is exactly what a future SMP (multi-core) milestone would start from, since it's the table that tells you how many CPUs exist.

## Next

The two large remaining milestones — preemptive scheduling and a real disk driver — stay queued for slower, multi-session work.
