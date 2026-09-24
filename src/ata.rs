//! Disks, over ATA PIO mode.
//!
//! `lspci` (Stage 16) found a PIIX3 IDE controller on this machine — this
//! module talks to it. ATA PIO ("Programmed I/O") is the oldest, simplest way
//! to move data on and off an IDE disk: no DMA, no interrupts, just port I/O
//! and a busy-wait, one 16-bit word at a time. Slow by modern standards, but
//! it's real disk access with no driver framework required.
//!
//! The primary IDE bus can carry two drives: the **master** (here: the disk
//! the kernel booted from) and the **slave** (an optional second "data" disk).
//!
//! ## Write safety is structural, not a convention
//!
//! The boot disk is the very image the kernel is running from, so nothing may
//! ever write to it by accident. There is exactly one write function,
//! `write_data_sector`, and it is *hard-wired to the slave drive*: the master
//! has no write path at all. Writing to the wrong disk isn't "guarded
//! against" — it's inexpressible.
//!
//! Every wait in here is a *bounded* poll: a missing or wedged drive returns
//! an error instead of freezing the kernel in a busy-loop.

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

/// How many status polls before we give up on a drive. Generous enough for a
/// slow emulated disk, finite so a missing drive can never hang the kernel.
const POLL_LIMIT: u32 = 2_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drive {
    Master,
    Slave,
}

impl Drive {
    /// Drive/head register value for LBA mode: bits 7 and 5 always set, bit 6
    /// = LBA, bit 4 = which drive; the low nibble carries LBA bits 24-27.
    fn lba_select(self, lba: u32) -> u8 {
        let base = match self {
            Drive::Master => 0xE0,
            Drive::Slave => 0xF0,
        };
        base | (((lba >> 24) & 0x0F) as u8)
    }
    /// Value used to select the drive for IDENTIFY (no LBA bits needed).
    fn identify_select(self) -> u8 {
        match self {
            Drive::Master => 0xA0,
            Drive::Slave => 0xB0,
        }
    }
}

unsafe fn inb(port: u16) -> u8 {
    Port::new(port).read()
}
unsafe fn outb(port: u16, value: u8) {
    Port::new(port).write(value)
}
unsafe fn inw(port: u16) -> u16 {
    Port::new(port).read()
}
unsafe fn outw(port: u16, value: u16) {
    Port::new(port).write(value)
}

/// The spec wants ~400ns after selecting a drive before touching it. Reading
/// the alternate status port is a standard trick to get that delay for free.
unsafe fn delay_400ns() {
    for _ in 0..4 {
        inb(ALT_STATUS);
    }
}

/// Poll until BSY clears. `None` on timeout.
unsafe fn wait_not_busy() -> Option<u8> {
    for _ in 0..POLL_LIMIT {
        let s = inb(COMMAND_STATUS);
        if s & STATUS_BSY == 0 {
            return Some(s);
        }
    }
    None
}

/// Poll until the drive is ready to transfer data (DRQ) — `Err` if it reports
/// an error or never gets there.
unsafe fn wait_drq() -> Result<(), &'static str> {
    for _ in 0..POLL_LIMIT {
        let s = inb(COMMAND_STATUS);
        if s & STATUS_ERR != 0 {
            return Err("drive reported an error");
        }
        if s & STATUS_BSY == 0 && s & STATUS_DRQ != 0 {
            return Ok(());
        }
    }
    Err("timed out waiting for the drive")
}

/// What a successful IDENTIFY DEVICE command tells us about the drive.
pub struct IdentifyInfo {
    pub model: [u8; 40],
    pub sectors_28bit: u32,
}

/// Ask a drive to identify itself. Returns `None` if there's no drive there,
/// or if it's not a plain ATA disk.
pub fn identify(drive: Drive) -> Option<IdentifyInfo> {
    unsafe {
        outb(DRIVE_HEAD, drive.identify_select());
        delay_400ns();
        outb(SECTOR_COUNT, 0);
        outb(LBA_LOW, 0);
        outb(LBA_MID, 0);
        outb(LBA_HIGH, 0);
        outb(COMMAND_STATUS, 0xEC); // IDENTIFY DEVICE

        // Status 0 (or a floating-bus 0xFF) means nothing is wired up here.
        let first = inb(COMMAND_STATUS);
        if first == 0 || first == 0xFF {
            return None;
        }
        wait_not_busy()?;

        // A real ATA disk leaves LBA mid/high at zero here; anything else
        // (notably an ATAPI device) signals itself differently.
        if inb(LBA_MID) != 0 || inb(LBA_HIGH) != 0 {
            return None;
        }
        wait_drq().ok()?;

        // The whole IDENTIFY response is exactly 256 sixteen-bit words.
        let mut raw = [0u16; 256];
        for word in raw.iter_mut() {
            *word = inw(DATA);
        }

        // Words 27..47 hold a 40-character model string, but ATA strings pack
        // each pair of characters high-byte-first within its word (the
        // opposite of a plain little-endian read).
        let mut model = [0u8; 40];
        for (i, word) in raw[27..47].iter().enumerate() {
            let bytes = word.to_be_bytes();
            model[i * 2] = bytes[0];
            model[i * 2 + 1] = bytes[1];
        }

        // Words 60-61: total addressable sectors in 28-bit LBA mode.
        let sectors_28bit = (raw[60] as u32) | ((raw[61] as u32) << 16);

        Some(IdentifyInfo {
            model,
            sectors_28bit,
        })
    }
}

/// Read one 512-byte sector via 28-bit LBA addressing.
pub fn read_sector(drive: Drive, lba: u32) -> Option<[u8; 512]> {
    unsafe {
        outb(DRIVE_HEAD, drive.lba_select(lba));
        delay_400ns();
        outb(SECTOR_COUNT, 1);
        outb(LBA_LOW, (lba & 0xFF) as u8);
        outb(LBA_MID, ((lba >> 8) & 0xFF) as u8);
        outb(LBA_HIGH, ((lba >> 16) & 0xFF) as u8);
        outb(COMMAND_STATUS, 0x20); // READ SECTORS (with retry)

        wait_drq().ok()?;

        // Sector data reads back in plain little-endian order.
        let mut sector = [0u8; 512];
        for i in 0..256 {
            let bytes = inw(DATA).to_le_bytes();
            sector[i * 2] = bytes[0];
            sector[i * 2 + 1] = bytes[1];
        }
        Some(sector)
    }
}

/// Write one 512-byte sector to the **data disk (slave drive) only**.
///
/// There is deliberately no equivalent for the master drive. After the data
/// is transferred we issue CACHE FLUSH (0xE7) and wait for it, so when this
/// returns `Ok` the drive has actually committed the sector, not just buffered
/// it.
pub fn write_data_sector(lba: u32, data: &[u8; 512]) -> Result<(), &'static str> {
    let drive = Drive::Slave;
    unsafe {
        outb(DRIVE_HEAD, drive.lba_select(lba));
        delay_400ns();
        outb(SECTOR_COUNT, 1);
        outb(LBA_LOW, (lba & 0xFF) as u8);
        outb(LBA_MID, ((lba >> 8) & 0xFF) as u8);
        outb(LBA_HIGH, ((lba >> 16) & 0xFF) as u8);
        outb(COMMAND_STATUS, 0x30); // WRITE SECTORS (with retry)

        wait_drq()?;
        for i in 0..256 {
            outw(DATA, u16::from_le_bytes([data[i * 2], data[i * 2 + 1]]));
        }

        outb(COMMAND_STATUS, 0xE7); // CACHE FLUSH
        let status = wait_not_busy().ok_or("timed out flushing the drive cache")?;
        if status & STATUS_ERR != 0 {
            return Err("drive reported an error after the write");
        }
        Ok(())
    }
}
