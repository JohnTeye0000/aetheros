# Devlog 008 — A read-only filesystem (ramdisk)

**Milestone:** the shell can now list and read files. `ls` shows what's there; `cat <file>` prints a file's contents.

## What a filesystem really is (and isn't, yet)

Strip away the disk hardware and a filesystem is one idea: **ask for bytes by name.** You say "give me `motd`" and you get its contents back, without knowing or caring where those bytes physically sit.

That naming-and-lookup layer is what this milestone adds. What it deliberately does *not* add yet is a real storage device. Our "disk" is a fixed table of files compiled straight into the kernel binary as static data — a **ramdisk**. A real OS gets its initial files from an *initrd* (initial RAM disk) the bootloader loads, or by reading blocks off a real disk through a driver. We have neither a disk driver nor an initrd loader yet, so baking the files in is the honest simplest version.

The important part is the **interface**, not the storage:

```rust
pub fn files() -> impl Iterator<Item = &File>   // what's here
pub fn read(name: &str) -> Option<&str>         // give me this one
```

Because `ls` and `cat` only talk to *that*, swapping the static table for a real disk-backed store later is invisible to them. That separation — a stable interface over a swappable implementation — is exactly how the real ones are layered (the "virtual filesystem" idea), just in miniature.

## How `ls` and `cat` work

Both are new arms in the shell's command `match`:

- **`ls`** iterates `fs::files()` and prints each file's size and name. The size is just `contents.len()` — no metadata structure yet, but it reads like a real `ls -l`.
- **`cat <file>`** calls `fs::read(name)`. `Some(contents)` → print them; `None` → `cat: <name>: no such file`. Handling the missing-file case is part of making it feel real.

Adding a file is now a one-line entry in the static table, and adding a command that uses the filesystem is a one-line `match` arm. The plumbing did its job.

## Why this is satisfying

It closes a loop. Earlier milestones each proved one mechanism in isolation. Now they stack: the **keyboard** types a command, the **shell** parses it, the **filesystem** answers, and the **screen driver** shows the result — a request flowing cleanly through four layers we built separately. That layering, more than any single feature, is what an operating system *is*.

## Next

A real disk-backed or initrd-loaded filesystem (so files aren't compiled in), and/or preemptive scheduling — the timer interrupt forcibly switching between tasks rather than tasks yielding voluntarily.
