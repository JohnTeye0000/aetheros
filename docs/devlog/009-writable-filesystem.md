# Devlog 009 — A read-write filesystem

**Milestone:** the ramdisk can now be changed at runtime. `write <file> <text>` creates or overwrites a file, `rm <file>` deletes one — and `ls`/`cat` see the changes immediately.

## From "baked in" to "alive"

Stage 8's filesystem was a fixed table compiled into the kernel — read-only by construction. This stage makes it dynamic, and the change is essentially "use the heap":

- the file list went from a `static` array to a `Vec<File>` on the heap,
- each file's name and contents went from `&'static str` (fixed, compile-time) to owned `String`s,
- and the whole thing sits behind a spinlock so it can be touched safely.

That's the payoff of Stage 5 (the heap) showing up again: once you can allocate at runtime, data structures that *grow and shrink* — like a list of files — become possible. A read-write filesystem is exactly that kind of structure.

## The four operations

A filesystem, at heart, is CRUD on named blobs:

- **list** — clone out `(name, size)` for each file so the shell can print them without holding the lock.
- **read(name)** — find by name, clone the contents back (`Option<String>`).
- **write(name, contents)** — if the name exists, replace its contents; otherwise push a new `File`. (Create-or-overwrite, like a real `>` redirect.)
- **remove(name)** — find its position and `Vec::remove` it; return whether it existed.

The shell wires each to a command: `ls`, `cat`, `write`, `rm`. Notice the demo: `write a hi` creates a file, `cat a` reads "hi" straight back, `rm motd` deletes a starter file, and `ls` confirms it's gone. The kernel's own heap is holding all of that.

## One honest design note

I clone data out of the filesystem (e.g. `read` returns an owned `String`) rather than handing back references. Returning a `&str` that borrows from inside a locked `Vec` fights the borrow checker hard — the reference would outlive the lock guard. Cloning sidesteps it: a little extra allocation in exchange for simple, obviously-correct code. In a kernel, "obviously correct" usually wins over "maximally efficient" until you have a measured reason otherwise.

## What's still missing (deliberately)

It's all in RAM, so it vanishes on reboot. Real persistence needs a **disk driver** (talk to an ATA/AHCI/virtio device), a way to lay files out in **blocks** on that disk, and a format to find them again — that's a filesystem *on storage*, a much bigger project. What we have is the in-memory model of the same idea, with a clean interface that a disk-backed version could later replace without the shell noticing.

## Next

Either a real device — a disk driver so files survive reboots — or preemptive scheduling, where the timer interrupt forcibly switches between tasks instead of tasks yielding.
