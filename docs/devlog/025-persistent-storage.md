# Day 25 — Files that survive power-off

Until today every file lived on the kernel heap and vanished at shutdown. Now `save` writes the whole filesystem to a **second disk** and the kernel restores it at boot.

## Design

- **Two disks.** The boot disk (primary master) is the image the kernel runs from; it stays read-only. A separate *data disk* (primary slave) holds saved files. Run QEMU with `-drive file=data.img,format=raw,if=ide,index=1` (a 1 MiB zero file works).
- **Writing to the wrong disk is inexpressible.** `ata.rs` has exactly one write function, `write_data_sector`, hard-wired to the slave. The master has no write path at all.
- **Bounded polling.** Every busy-wait has a limit, so a missing/wedged drive returns an error instead of hanging the kernel.
- **CACHE FLUSH** (0xE7) after each write, so `Ok` means the drive committed it.

## AFS1 format

```
sector 0   "AFS1" | blob_len u32 | fnv1a checksum u32 | file_count u32
sector 1.. per file: name_len u16 | content_len u32 | name | content
```

- The blob is written **first**, the header **last** — a power cut mid-save never leaves a header promising data that isn't there.
- `load` validates magic, sizes, checksum, entry bounds (checked arithmetic), UTF-8 and file count *before* touching memory. Any failure leaves the current filesystem unchanged.

## Verified in QEMU

| Scenario | Result |
|---|---|
| write + `save`, reboot | `[ disk ] restored 4 files`, `cat` returns the contents |
| flip one byte in the data sector | `checksum mismatch - refusing to load corrupt data`, seed files intact |
| no data disk | boot says RAM-only; `load` → `no data disk attached` |

## Known limits

Whole-filesystem rewrite on each save (no incremental updates), 128 KiB cap, 28-bit LBA, no journaling beyond the header-last ordering, and a checksum that detects corruption but is not cryptographic.
