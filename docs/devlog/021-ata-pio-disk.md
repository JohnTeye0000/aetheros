# Devlog 021 — A real disk driver (ATA PIO, read-only)

**Milestone:** the kernel talks to an actual storage device. `disk` identifies the drive `lspci` found back in Stage 16, then reads a genuine sector off it and checks its boot signature.

## The oldest interface still works

ATA PIO ("Programmed I/O") is the original way software moved bytes on and off an IDE disk — no DMA, no interrupts, nothing clever: write a command to a port, poll a status bit until the drive says it has data, then read that data one 16-bit word at a time through a single port. It's slow by any modern measure, and every PC built since the 1980s still understands it. Compared to the HPET's memory-mapped registers from Stage 20, this is a full return to the port-I/O style of the PIT, RTC, and PCI config space — same mechanism, new device.

Two commands do the whole job:

- **IDENTIFY DEVICE (`0xEC`)** — ask the drive to describe itself: a 40-character model string, and (among 256 words of other data) the total sector count in 28-bit LBA mode.
- **READ SECTORS (`0x20`)** — hand over a 28-bit sector number (LBA) and get exactly 512 bytes back.

Both follow the same shape: select the drive, write the command parameters, write the command byte, poll the status register until the drive raises "data request" (or reports an error), then drain the data port.

## A byte-order quirk worth knowing

Sector *data* reads back in ordinary little-endian order — each 16-bit word's low byte is simply the next byte of the sector, exactly what you'd expect. The IDENTIFY *string fields* don't: ATA packs each pair of characters with the high byte of the word first, the reverse of the data words. Get this backwards and a real model string turns into scrambled bytes. The proof it's right: `"QEMU HARDDISK"` — QEMU's actual virtual-disk identity string — reads back perfectly clean, not reversed or interleaved.

## A verification hiccup that wasn't a kernel bug

The first test run came back with the shell sitting at an empty prompt — no `disk` output, no error, nothing. It looked exactly like a hang. But the plain boot-verify (no injected keystrokes) still reached the prompt cleanly with zero panics, and the screenshot showed `aether>` with *nothing typed after it at all* — not even the echoed characters. That ruled out the kernel: if `disk` itself had hung mid-command, the typed letters would still show up on screen (echoing happens before the command runs). The actual cause was outside the kernel entirely — QEMU running slower on the host this session, so the scripted keystrokes were injected before the emulator had even finished booting to the prompt. Slowing down the test's timing was the fix, not touching a single line of kernel code. Telling those two failure modes apart — "the kernel hung" versus "my test harness was too fast for a slow host" — is its own kind of debugging skill, and the tell is always the same: check what actually reached the screen before assuming the code is at fault.

## Deliberately read-only

There is no write path in this driver at all — not "write support with a safety flag," just no `WRITE SECTORS` code anywhere in the file. The disk behind these ports is the very image this kernel booted from, so the only way to guarantee it can't corrupt itself is to make writing structurally impossible, not merely avoided by convention.

## Proof

```
primary master: "QEMU HARDDISK"
1485 sectors (742 KiB, 28-bit LBA)
sector 0: last bytes 0x55 0xaa  boot signature: valid (0x55AA)
```

`0x55AA` is the real, spec-defined signature every boot sector ends with — and this is *this kernel's own* boot sector, read back through a driver it wrote for itself, over a bus a different subsystem (PCI enumeration) discovered three stages ago.

## Next

The two large remaining milestones — preemptive scheduling and a real, disk-backed filesystem (finally putting this driver to use for more than reading one sector) — stay queued for slower, multi-session work.
