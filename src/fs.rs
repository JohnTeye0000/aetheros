//! A tiny in-memory filesystem (a "ramdisk") — now read-write.
//!
//! A real OS reads files off a disk or an "initrd" the bootloader hands it. We
//! still don't have a disk driver, so our files live entirely in RAM — but now
//! they live on the **heap** (Stage 5), in a growable `Vec` of `File`s with
//! owned `String` contents. That means files can be created, overwritten, and
//! deleted at runtime, not just read.
//!
//! The trade-off is honest: because there's no real storage device, everything
//! here is lost on reboot. Persisting across reboots needs a disk driver, which
//! is a later milestone. What this proves is the read-write *interface* — and
//! that the kernel's own heap is solid enough to back it.

use alloc::string::String;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;

/// One file: an owned name and owned contents (both on the heap).
pub struct File {
    pub name: String,
    pub contents: String,
}

lazy_static! {
    /// The whole "disk", behind a spinlock. Seeded with a few starter files.
    static ref FILES: Mutex<Vec<File>> = Mutex::new(seed());
}

/// The files the ramdisk starts with at boot.
fn seed() -> Vec<File> {
    let mut files = Vec::new();
    push(&mut files, "motd",
        "Welcome to AetherOS.\nAn x86-64 operating system built from scratch, one commit at a time.\n");
    push(&mut files, "readme.txt",
        "AetherOS is a from-scratch kernel written in Rust.\nTry: write notes hello world | ls | cat notes | rm notes\n");
    push(&mut files, "stages.txt",
        "boot -> exceptions -> timer -> keyboard -> paging -> heap -> tasks -> shell -> fs\nEvery layer above was written by hand.\n");
    files
}

fn push(files: &mut Vec<File>, name: &str, contents: &str) {
    files.push(File {
        name: String::from(name),
        contents: String::from(contents),
    });
}

/// List every file as `(name, size_in_bytes)`. Returns owned copies so callers
/// don't have to hold the filesystem lock while they print.
pub fn list() -> Vec<(String, usize)> {
    FILES
        .lock()
        .iter()
        .map(|f| (f.name.clone(), f.contents.len()))
        .collect()
}

/// Read a file's contents by name (a copy), or `None` if it doesn't exist.
pub fn read(name: &str) -> Option<String> {
    FILES
        .lock()
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.contents.clone())
}

/// Create a file, or overwrite it if the name already exists.
pub fn write(name: &str, contents: &str) {
    let mut files = FILES.lock();
    if let Some(file) = files.iter_mut().find(|f| f.name == name) {
        file.contents = String::from(contents);
    } else {
        files.push(File {
            name: String::from(name),
            contents: String::from(contents),
        });
    }
}

/// Copy out every file as `(name, contents)` — used by the persistence layer.
pub fn export() -> Vec<(String, String)> {
    FILES
        .lock()
        .iter()
        .map(|f| (f.name.clone(), f.contents.clone()))
        .collect()
}

/// Replace the entire filesystem with the given files (used after a validated
/// load from disk).
pub fn replace_all(new_files: Vec<(String, String)>) {
    let mut files = FILES.lock();
    files.clear();
    for (name, contents) in new_files {
        files.push(File { name, contents });
    }
}

/// Delete a file. Returns `true` if it existed.
pub fn remove(name: &str) -> bool {
    let mut files = FILES.lock();
    if let Some(pos) = files.iter().position(|f| f.name == name) {
        files.remove(pos);
        true
    } else {
        false
    }
}
