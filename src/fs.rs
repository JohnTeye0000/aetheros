//! A tiny read-only in-memory filesystem (a "ramdisk").
//!
//! A real OS reads files off a disk or an "initrd" (initial RAM disk) the
//! bootloader hands it. We don't have a disk driver yet, so our filesystem is
//! the simplest thing that still teaches the idea: a fixed set of named files
//! whose contents are **compiled into the kernel image** as static data.
//!
//! What matters is the *abstraction* it introduces: code (and the shell's `ls`
//! / `cat`) now asks for a file *by name* and gets bytes back, without caring
//! where those bytes physically live. Swapping this static table for a real
//! disk-backed store later is then just an implementation detail behind the
//! same interface.

/// One file: a name and its contents.
pub struct File {
    pub name: &'static str,
    pub contents: &'static str,
}

/// The whole "disk" — baked into the kernel binary. Read-only by construction.
static FILES: &[File] = &[
    File {
        name: "motd",
        contents: "Welcome to AetherOS.\nAn x86-64 operating system built from scratch, one commit at a time.\n",
    },
    File {
        name: "readme.txt",
        contents: "AetherOS is a from-scratch kernel written in Rust.\nType 'help' for shell commands; 'ls' to list files.\n",
    },
    File {
        name: "stages.txt",
        contents: "boot -> exceptions -> timer -> keyboard -> paging -> heap -> tasks -> shell -> fs\nEvery layer above was written by hand.\n",
    },
];

/// Iterate over every file in the ramdisk.
pub fn files() -> impl Iterator<Item = &'static File> {
    FILES.iter()
}

/// Look up a file's contents by name. Returns `None` if there's no such file.
pub fn read(name: &str) -> Option<&'static str> {
    FILES.iter().find(|f| f.name == name).map(|f| f.contents)
}
