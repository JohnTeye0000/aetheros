//! A real disk, over ATA PIO mode.
//!
//! `lspci` (Stage 16) already found a PIIX3 IDE controller on this machine —
//! this module actually talks to it. ATA PIO ("Programmed I/O") is the
//! oldest, simplest way to move data on and off an IDE disk: no DMA, no
//! interrupts, just port I/O and a busy-wait, one 16-bit word at a time. Slow
//! by modern standards, but it's real disk access with no driver framework
//! required — the same handful of ports every PC has used since the 1980s.
//!
//! This module is deliberately **read-only**. The disk behind these ports is
//! the very image the kernel booted from; there is no write path here at all,
//! so there is nothing that could corrupt it.

use x86_64::instructions::port::Port;

// The "primary" ATA bus's fixed port block.
const DATA: u16 = 0x1F0;
const SECTOR_COUNT: u16 = 0x1F2;
const LBA_LOW: u16 = 0x1F3;
const LBA_MID: u16 = 0x1F4;
const LBA_HIGH: u16 = 0x1F5;
const DRIVE_HEAD: u16 = 0x1F6;
const COMMAND_STATUS: u16 = 0x1F7;
const ALT_STATUS: u16 = 0x3F6; // a second copy of the status register

const STATUS_ERR: u8 = 1 << 0;
const STATUS_DRQ: u8 = 1 << 3; // "data request" — the drive has data ready
const STATUS_BSY: u8 = 1 << 7;

unsafe fn inb(port: u16) -> u8 {
    Port::new(port).read()
}
unsafe fn outb(port: u16, value: u8) {
    Port::new(port).write(value)
}
unsafe fn inw(port: u16) -> u16 {
    Port::new(port).read()
}

/// The spec wants ~400ns after selecting a drive before touching it. Reading
/// the alternate status port is a standard trick to get that delay for free:
/// the read itself takes roughly the right amount of time, and — unlike
/// reading the main status port — it doesn't clear a pending interrupt flag.
unsafe fn delay_400ns() {
    for _ in 0..4 {
        inb(ALT_STATUS);
    }
}

/// What a successful IDENTIFY DEVICE command tells us about the drive.
pub struct IdentifyInfo {
    pub model: [u8; 40],
    pub sectors_28bit: u32,
}

/// Ask the primary master drive to identify itself. Returns `None` if there's
/// no drive there, or if it's not a plain ATA disk (an ATAPI drive — a CD/DVD
/// drive wired to the same bus — leaves a different signature and we don't
/// try to handle it here).
pub fn identify_primary_master() -> Option<IdentifyInfo> {
    unsafe {
        outb(DRIVE_HEAD, 0xA0); // select "master", no LBA bits needed yet
        delay_400ns();
        outb(SECTOR_COUNT, 0);
        outb(LBA_LOW, 0);
        outb(LBA_MID, 0);
        outb(LBA_HIGH, 0);
        outb(COMMAND_STATUS, 0xEC); // IDENTIFY DEVICE

        // A status of 0 means the bus has no drive wired to it at all.
        if inb(COMMAND_STATUS) == 0 {
            return None;
        }

        // Poll until the drive is done processing the command.
        while inb(COMMAND_STATUS) & STATUS_BSY != 0 {}

        // A real ATA disk leaves LBA mid/high at zero here; anything else
        // (notably an ATAPI device) signals itself differently.
        if inb(LBA_MID) != 0 || inb(LBA_HIGH) != 0 {
            return None;
        }

        loop {
            let status = inb(COMMAND_STATUS);
            if status & STATUS_ERR != 0 {
                return None;
            }
            if status & STATUS_DRQ != 0 {
                break;
            }
        }

        // The whole IDENTIFY response is exactly 256 sixteen-bit words.
        let mut raw = [0u16; 256];
        for word in raw.iter_mut() {
            *word = inw(DATA);
        }

        // Words 27..47 hold a 40-character model string, but ATA strings pack
        // each pair of characters high-byte-first within its word (the
        // opposite of a plain little-endian read) — a well-known quirk of the
        // spec, not a mistake in how we're reading it.
        let mut model = [0u8; 40];
        for (i, word) in raw[27..47].iter().enumerate() {
            let bytes = word.to_be_bytes();
            model[i * 2] = bytes[0];
            model[i * 2 + 1] = bytes[1];
        }

        // Words 60-61 give the total addressable sector count in 28-bit LBA
        // mode, as a little-endian 32-bit value split across the two words.
        let sectors_28bit = (raw[60] as u32) | ((raw[61] as u32) << 16);

        Some(IdentifyInfo {
            model,
            sectors_28bit,
        })
    }
}

/// Read one 512-byte sector from the primary master via 28-bit LBA addressing
/// — the classic "READ SECTORS" command, no DMA, no interrupts.
pub fn read_sector(lba: u32) -> Option<[u8; 512]> {
    unsafe {
        // 0xE0 selects the master drive in LBA mode; the top nibble of a
        // 28-bit LBA rides along in the low bits of this same register.
        outb(DRIVE_HEAD, 0xE0 | (((lba >> 24) & 0x0F) as u8));
        delay_400ns();
        outb(SECTOR_COUNT, 1);
        outb(LBA_LOW, (lba & 0xFF) as u8);
        outb(LBA_MID, ((lba >> 8) & 0xFF) as u8);
        outb(LBA_HIGH, ((lba >> 16) & 0xFF) as u8);
        outb(COMMAND_STATUS, 0x20); // READ SECTORS (with retry)

        loop {
            let status = inb(COMMAND_STATUS);
            if status & STATUS_ERR != 0 {
                return None;
            }
            if status & STATUS_BSY == 0 && status & STATUS_DRQ != 0 {
                break;
            }
        }

        // Sector data reads back in plain little-endian order (unlike the
        // model string above) — each word's low byte is simply the next byte
        // of the sector.
        let mut sector = [0u8; 512];
        for i in 0..256 {
            let bytes = inw(DATA).to_le_bytes();
            sector[i * 2] = bytes[0];
            sector[i * 2 + 1] = bytes[1];
        }
        Some(sector)
    }
}
