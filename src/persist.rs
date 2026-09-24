//! Saving the filesystem to disk and restoring it after a reboot.
//!
//! Until now files lived only on the kernel heap and vanished at power-off.
//! This module serialises the whole in-memory filesystem onto the **data
//! disk** (the second IDE drive) and reads it back.
//!
//! ## On-disk format ("AFS1")
//!
//! ```text
//! sector 0   header: "AFS1" | blob_len:u32 | checksum:u32 | file_count:u32
//! sector 1.. blob:   per file:  name_len:u16 | content_len:u32 | name | content
//! ```
//!
//! All integers little-endian. The checksum is FNV-1a over the blob. Loading
//! **refuses** anything that doesn't validate — wrong magic, oversized length,
//! checksum mismatch, malformed entries, invalid UTF-8 — and leaves the
//! in-memory filesystem untouched, so a corrupt or foreign disk can never
//! clobber what's already loaded.
//!
//! Save order matters: the blob sectors are written first and the header last,
//! so a power cut mid-save leaves the *old* header (and thus a self-consistent
//! old filesystem or a cleanly-rejected mismatch) rather than a header that
//! promises data that isn't there yet.

use crate::ata::{self, Drive};
use alloc::string::String;
use alloc::vec::Vec;

const MAGIC: &[u8; 4] = b"AFS1";
/// Hard cap on the saved image (header + blob), in sectors — 128 KiB.
const MAX_SECTORS: u32 = 256;

/// FNV-1a, a tiny well-known non-cryptographic hash: enough to catch
/// corruption and partial writes, which is all we need here.
fn checksum(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in data {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

fn serialize(files: &[(String, String)]) -> Vec<u8> {
    let mut blob = Vec::new();
    for (name, contents) in files {
        blob.extend_from_slice(&(name.len() as u16).to_le_bytes());
        blob.extend_from_slice(&(contents.len() as u32).to_le_bytes());
        blob.extend_from_slice(name.as_bytes());
        blob.extend_from_slice(contents.as_bytes());
    }
    blob
}

fn deserialize(blob: &[u8], expected: usize) -> Result<Vec<(String, String)>, &'static str> {
    let mut files = Vec::new();
    let mut i = 0;
    while i < blob.len() {
        if i + 6 > blob.len() {
            return Err("truncated entry header");
        }
        let name_len = u16::from_le_bytes([blob[i], blob[i + 1]]) as usize;
        let content_len =
            u32::from_le_bytes([blob[i + 2], blob[i + 3], blob[i + 4], blob[i + 5]]) as usize;
        i += 6;
        let end = i
            .checked_add(name_len)
            .and_then(|n| n.checked_add(content_len))
            .ok_or("entry length overflow")?;
        if end > blob.len() {
            return Err("entry runs past the end of the data");
        }
        let name = String::from_utf8(blob[i..i + name_len].to_vec())
            .map_err(|_| "file name is not valid UTF-8")?;
        let contents = String::from_utf8(blob[i + name_len..end].to_vec())
            .map_err(|_| "file contents are not valid UTF-8")?;
        files.push((name, contents));
        i = end;
    }
    if files.len() != expected {
        return Err("file count does not match the header");
    }
    Ok(files)
}

/// Sector count of the data disk (second drive), if one is attached.
pub fn data_disk_sectors() -> Option<u32> {
    ata::identify(Drive::Slave).map(|i| i.sectors_28bit)
}

/// Write the whole filesystem to the data disk. Returns `(files, sectors)`.
pub fn save() -> Result<(usize, u32), &'static str> {
    let disk_sectors = data_disk_sectors().ok_or("no data disk attached")?;
    let files = crate::fs::export();
    let blob = serialize(&files);
    let blob_sectors = ((blob.len() + 511) / 512) as u32;
    let total = 1 + blob_sectors;
    if total > MAX_SECTORS || total > disk_sectors {
        return Err("filesystem too large for the data disk");
    }

    // Blob first...
    for n in 0..blob_sectors {
        let mut sector = [0u8; 512];
        let start = (n as usize) * 512;
        let end = (start + 512).min(blob.len());
        sector[..end - start].copy_from_slice(&blob[start..end]);
        ata::write_data_sector(1 + n, &sector)?;
    }

    // ...header last, so the header never describes data that isn't there yet.
    let mut header = [0u8; 512];
    header[0..4].copy_from_slice(MAGIC);
    header[4..8].copy_from_slice(&(blob.len() as u32).to_le_bytes());
    header[8..12].copy_from_slice(&checksum(&blob).to_le_bytes());
    header[12..16].copy_from_slice(&(files.len() as u32).to_le_bytes());
    ata::write_data_sector(0, &header)?;

    Ok((files.len(), total))
}

/// Replace the in-memory filesystem with the one saved on the data disk.
/// Returns the number of files restored. On ANY validation failure nothing in
/// memory is changed.
pub fn load() -> Result<usize, &'static str> {
    let disk_sectors = data_disk_sectors().ok_or("no data disk attached")?;
    let header = ata::read_sector(Drive::Slave, 0).ok_or("could not read sector 0")?;
    if &header[0..4] != MAGIC {
        return Err("no saved filesystem on the data disk (bad magic)");
    }
    let blob_len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    let want_sum = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    let count = u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as usize;

    let blob_sectors = ((blob_len + 511) / 512) as u32;
    if 1 + blob_sectors > MAX_SECTORS || 1 + blob_sectors > disk_sectors {
        return Err("header describes an impossible size");
    }

    let mut blob = Vec::with_capacity(blob_len);
    for n in 0..blob_sectors {
        let sector = ata::read_sector(Drive::Slave, 1 + n).ok_or("read error while loading")?;
        let take = (blob_len - blob.len()).min(512);
        blob.extend_from_slice(&sector[..take]);
    }

    if checksum(&blob) != want_sum {
        return Err("checksum mismatch - refusing to load corrupt data");
    }
    let files = deserialize(&blob, count)?;
    let n = files.len();
    crate::fs::replace_all(files);
    Ok(n)
}
