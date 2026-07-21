# Devlog 020 — The HPET, and the kernel's first memory-mapped device

**Milestone:** the kernel reads a real hardware timer, the **HPET**, using a fundamentally different access mechanism than every device so far — and along the way, Rust's own safety checks caught a genuine bug before it became a mystery.

## Two ways to talk to hardware

Every device this kernel has touched — the PIT (Stage 2), the CMOS RTC (Stage 14), the keyboard (Stage 3), PCI configuration space (Stage 16) — uses **port I/O**: special `in`/`out` CPU instructions addressing a small, separate 64 KB port space. The HPET uses the other mechanism entirely: **memory-mapped I/O (MMIO)**. Its registers sit at an ordinary physical address, and you read or write them with plain memory loads and stores — no special instruction at all. The *only* thing that makes it a "device register" instead of RAM is that the chipset, not DRAM, answers when the CPU touches that address.

That also means finding the address is the whole problem. Nothing hands it to you directly — which is exactly why Stage 18's RSDT walk mattered: the HPET's ACPI table (found by signature, `"HPET"`) contains its base physical address as one of its fields.

## The bug the type system caught

The first version read that base-address field the same way `hpet.rs` reads the HPET's *own* registers — a plain `read_volatile::<u64>`. It panicked immediately:

```
unsafe precondition(s) violated: ptr::read_volatile requires that the pointer argument is aligned
```

The distinction that broke: a `read_volatile::<u64>` requires the pointer to be 8-byte aligned — real hardware guarantees that for MMIO registers (a device's base address is conventionally page-aligned, so `base + 0xF0` is always a multiple of 8). But the field I was reading wasn't a device register — it was a **field packed inside an ACPI table**, and the ACPI spec makes no such promise. Table fields sit wherever the byte layout puts them; this one happened to land 4 bytes off an 8-byte boundary.

The fix: `core::ptr::read_unaligned` instead of `read_volatile` for that one field — the right tool for "this address isn't guaranteed aligned" versus "this is a live register that must not be cached or reordered." They're solving two different problems, and mixing them up is exactly the kind of bug that's brutal to find by hand (it would show up as garbage data or a crash somewhere downstream, far from the actual mistake) but which Rust's debug-mode alignment check catches immediately, with a precise error and line number. This is the second time this project found a real bug through a build-time or debug-time safety net rather than by staring at wrong output (the first was Stage 3's masked keyboard IRQ) — a pattern worth noticing.

## What we do once we have the address

Read the **General Capabilities** register to learn the counter's width, how many comparators it has, and its **tick period** in femtoseconds (10⁻¹⁵ s — chosen so the period is always a whole number, regardless of the actual frequency). Check the **Configuration** register's enable bit — some firmware leaves the counter switched off until an OS asks for it, so if it's off, we turn it on ourselves. Then read the **Main Counter** twice, a short spin in between, to prove it's actually moving.

## The proof is arithmetic, not just numbers on a screen

```
tick period: 10000000 fs (100.000 MHz)
counter: 19400 -> 28284960  (282655600 ns elapsed)
```

10,000,000 femtoseconds is 10 nanoseconds — a 100 MHz tick rate, which is exactly what's printed, computed independently via integer arithmetic (no floating point; this target deliberately runs without SSE/hardware float, so plain `u64` math is the right tool, not just a workaround). And `(28284960 − 19400) × 10 ns = 282,655,600 ns` — precisely the number the kernel printed. That match isn't decoration; it's the elapsed-time calculation checking its own work in front of you.

## Next

The two large remaining milestones — preemptive scheduling and a real disk driver — stay queued for slower, multi-session work.
