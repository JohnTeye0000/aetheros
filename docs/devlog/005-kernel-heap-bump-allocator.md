# Devlog 005 — A kernel heap (bump allocator, written from scratch)

**Milestone:** `Box`, `Vec`, and `String` now work inside the kernel. We have a real heap, managed by an allocator we wrote ourselves.

## Why this needed the previous milestone

Heap types like `Vec` don't know their size at compile time — they grow at runtime. That requires *dynamic allocation*: a chunk of memory the program can carve up and hand back on demand. Two ingredients:

1. **Memory that exists.** That's where Stage 4 (paging) pays off — we map a region of virtual pages onto real physical frames so there's actual writable RAM behind our chosen heap address (`0x4444_4444_0000`, 100 KiB).
2. **An allocator** that tracks which parts of that region are free, and a way to tell Rust to use it: the `#[global_allocator]`. Once it's registered, every `Box::new`, `Vec::push`, etc. routes through it automatically.

## The bump allocator

I wrote the simplest allocator there is — a **bump allocator**. It keeps one number, `next`, starting at the bottom of the heap. To allocate:

1. round `next` up to the requested alignment,
2. hand back that address,
3. bump `next` past the block.

That's it — allocation is a pointer addition, which is as fast as it gets.

The honest tradeoff is freeing. A bump allocator only remembers `next`, so it *can't* free an individual block. It counts how many allocations are live and only resets `next` to the bottom when that count hits zero (everything freed at once). So a long-running `Vec` that grows and shrinks repeatedly would "leak" until all allocations drop. That's a real limitation — a free-list allocator that reclaims individual blocks is a future milestone — but the bump allocator is the clearest way to *understand* what a global allocator actually is.

One Rust detail worth knowing: `GlobalAlloc`'s methods take `&self`, but the allocator must mutate `next`. We can't put the data behind a `Mutex` and implement `GlobalAlloc` on it directly (the "orphan rule" forbids implementing a foreign trait on a foreign type), so there's a tiny `Locked<T>` wrapper around `spin::Mutex` that we own and *can* implement the trait on.

## A detour that turned into an upgrade

The plan was to pull in the `linked_list_allocator` crate. But the internet dropped mid-build — `cargo` couldn't reach crates.io. Rather than wait, I wrote the allocator by hand instead.

That turned out better on every axis: it builds with **no network and no dependency**, it's **simpler to read**, and — for a project whose whole point is understanding — *writing* the allocator beats *importing* one. "I implemented a heap allocator" is a stronger sentence than "I added a crate."

## Proof

The serial log shows a `Box<u64>` living at the heap's base address, a `Vec` of 100 numbers summing to 5050, and a `String` — all allocated and freed through our allocator, with no panic.

## Next

Multitasking: letting the kernel run more than one thing at a time. This is the big one — a scheduler, tasks, and concurrency — and it builds directly on having a heap to allocate task state from.
