# Devlog 004 — Paging & virtual memory

**Milestone:** the kernel can now read and rewrite its own page tables — the machinery behind virtual memory. This is the biggest conceptual jump so far.

## The core idea: addresses lie

Every address your code uses is a **virtual address**. It is *not* where the data physically sits in RAM. Between the two is a hardware translation step: on every single memory access, the CPU looks up the virtual address in a set of tables (the **page tables**) to find the real **physical** address.

Why bother with this indirection? Because it's the foundation of almost everything an OS does:

- **Isolation** — give each program its own page tables and they literally cannot see each other's memory.
- **The illusion of contiguous memory** — scattered physical frames can be mapped to look like one continuous block.
- **Protection** — a page can be marked read-only or no-execute.

## How translation works (the 4-level tree)

A 64-bit virtual address is sliced into four 9-bit indexes plus a 12-bit offset. The CPU walks a 4-level tree: the top register (`CR3`) points at the level-4 table, an index picks an entry pointing at a level-3 table, and so on down to a level-1 entry that names the physical frame. The bottom 12 bits select the byte within that 4 KiB frame.

We don't do that walk by hand — the `x86_64` crate's `OffsetPageTable` does it. But there's a chicken-and-egg problem to solve first.

## The bootstrap problem (and the offset trick)

To *read* a page table, we need its address. But page tables store **physical** addresses, and the CPU can only dereference **virtual** ones. So how do we follow a physical pointer?

The bootloader solves it for us. With the `map_physical_memory` feature, it maps *all* of physical RAM into virtual space at a fixed offset. So physical address `p` is readable at virtual address `offset + p`. Hand `OffsetPageTable` that offset and it can follow any physical pointer in the tables. `CR3` gives us the physical address of the top table; `offset + CR3` gives us a pointer we can actually use.

That's what `memory::init` sets up, and why the serial log prints `phys offset 0x018000000000 -> PhysAddr(0x0)`: the offset itself maps straight to physical address 0.

## Proving it both ways

- **Reading:** I translated three addresses and printed virtual→physical. The VGA buffer is identity-mapped (`0xb8000 -> 0xb8000`); a stack variable lives at some unrelated physical frame (`...1cb0 -> 0x279cb0`). The kernel computed these by walking the tables.
- **Writing:** I mapped a brand-new, never-used virtual page (`0xdeadbeef000`) onto the VGA buffer's physical frame, then wrote "New!" through that new page. The text appeared on screen — through a mapping the kernel created from scratch. Creating a mapping can require a new page table, so this also needed a **frame allocator** (`BootInfoFrameAllocator`) that hands out unused physical frames from the bootloader's memory map.

## One thing to take away

Physical memory is a flat array of frames. Virtual memory is a *story* the page tables tell about that array. Today the kernel learned to both read and write that story. Next we use it to carve out a **heap**, which finally unlocks `Box`, `Vec`, and `String` — dynamic data structures inside the kernel.
