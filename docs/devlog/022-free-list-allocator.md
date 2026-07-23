# Devlog 022 — Upgrading to a free-list allocator

**Milestone:** the kernel's heap allocator can now reclaim individual blocks. Free something, allocate something the same size, and you get back the exact address you just freed — a real capability the original design flagged as a limitation seventeen stages ago.

## The debt this pays off

Stage 5's devlog was honest about what it was building: "a bump allocator... can't free an individual block, because it only remembers one pointer. So it counts live allocations and only resets `next` to the start once that count hits zero." That was the right first allocator to write — the clearest possible way to explain what a global allocator even is — but it meant the heap effectively never shrank in a long-running kernel unless *everything* on it was freed at the exact same moment. Today's upgrade removes that limitation.

## From one pointer to a list of free blocks

A **free-list allocator** keeps not one number but a linked list of every currently-free region. The trick that makes this cheap: each free block's bookkeeping — its size, and a pointer to the next free block — is stored **inside the block itself**. Freeing memory costs nothing extra, because the freed memory *is* the record of having been freed.

- **Allocating** walks the list looking for the first block big enough (this project's version, like the classic design, uses "first fit" — take the first one that works, not the smallest one that would waste the least space). If the block is bigger than needed, the unused tail becomes its own new, smaller free block instead of being wasted.
- **Freeing** just writes a fresh `ListNode` into the returned memory and stitches it onto the front of the list. No search required to give memory back.

## Why the proof is convincing

`heaptest` allocates a byte, remembers its address, frees it, then allocates another byte and checks the address again:

```
allocated at 0x444444440770, then freed it
allocated again (same size) at 0x444444440770
same address, reused - free-list allocator reclaims individual blocks
```

That match is not a coincidence to be taken on faith — it follows directly from the mechanism: freeing pushed that exact block onto the *front* of the list, so the next allocation's first-fit search finds it immediately, before checking anywhere else. A bump allocator could never produce this result unless the entire heap had just been emptied; this allocator produces it on demand, for one block at a time.

## Keeping `mem` honest through the swap

The `mem` command's meaning needed care during the rewrite. A bump allocator's "bytes in use" was trivial — the distance the one pointer had moved. A free-list allocator doesn't track that number directly, so `heap_stats()` now computes it by **walking the free list and summing what's left over**, then reporting `used = total - free`. That's a small but deliberate choice: recomputing straight from the list's actual current state each time `mem` is called, rather than maintaining a separate running counter that could quietly drift out of sync with reality if a bug ever crept in elsewhere.

## Verifying nothing broke

Because this replaced the allocator behind *everything* built on the heap since Stage 5 — the filesystem's `Vec<File>`, the shell's command history, every `Box`/`Vec`/`String` in the boot sequence — the real risk wasn't the new code, it was silently breaking old code. The Stage 5 boot demo (`Box<u64>` at a specific address, a `Vec` summing to 5050, a `String`) came back byte-for-byte identical to every prior run, and `ls`/`mem`/`cp`/`history` all still work exactly as before. The new allocator is a drop-in replacement in the truest sense: same interface, same guarantees, more capability underneath.

## Next

The two large remaining milestones — preemptive scheduling and a real, disk-backed filesystem — stay queued for slower, multi-session work. The disk driver from Stage 21 is read-only by design; giving the filesystem real persistence would mean adding a `WRITE SECTORS` path deliberately, not by accident.
