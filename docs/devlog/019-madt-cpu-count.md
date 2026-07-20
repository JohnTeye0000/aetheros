# Devlog 019 — Decoding the MADT: how many CPUs does this machine have?

**Milestone:** `acpi` now decodes the **MADT**'s contents — not just finds and checksums it, but reads what's inside — to answer a question a real kernel genuinely needs answered: how many CPU cores exist, and where's the I/O APIC?

## Why this is the natural next layer

Stage 18 left off pointing at exactly this: the RSDT/XSDT walk finds tables by name, but doesn't look inside any of them. Every ACPI table has its own internal format once you're past the shared 36-byte header, and the **MADT** (Multiple APIC Description Table, signature `"APIC"`) is one of the most useful ones to actually parse, because its contents are the only place an x86 OS learns how many processors it's running on — there's no other register or instruction that just tells you.

## A stream of self-describing entries

After the MADT's header and two fixed fields (the local APIC's physical address, and a flags word), the rest of the table is a back-to-back stream of entries, each shaped the same way: a type byte, a length byte, then type-specific data. That `(type, length)` pair is the whole trick — even if you don't understand entry type 47, its length byte tells you exactly how many bytes to skip to reach the next one. That's what "forward-compatible format" means in practice: you can walk right past entries you don't decode, instead of needing to understand every one that could ever exist.

I only decode the two entry types this milestone actually needs:

- **Type 0 — Processor Local APIC.** Carries an APIC ID and a flags word whose bit 0 says whether the core is *enabled*. A MADT can technically list disabled/reserved entries (hardware the firmware knows about but doesn't want the OS to use), so counting cores means checking that bit, not just counting entries.
- **Type 1 — I/O APIC.** Its own ID, physical address, and the first interrupt number ("global system interrupt") it's responsible for routing.

Everything else — interrupt source overrides, NMI entries, and so on — gets skipped by length, exactly as the format intends.

## What came back, and why it's the right answer

```
MADT: 1 CPU core(s) (APIC ids: 0)
MADT: I/O APIC id 0 @ 0xfec00000 (GSI base 0)
```

Both numbers check out independently. QEMU's default machine boots one virtual core unless you explicitly ask for `-smp N`, so "1 CPU core, APIC id 0" is exactly right for how this kernel is being run. And `0xfec00000` isn't a value we computed — it's the fixed, standard physical address every x86 PC places its I/O APIC at, so seeing that exact number come out of a generic byte-parsing loop is a real confirmation the offsets and field widths are correct, not a lucky guess.

## Where this points

We're not doing anything with this information yet — no second core is woken up, no interrupt is rerouted through the I/O APIC. But "read the MADT, count the enabled local APICs" is the literal first step of bringing up a second CPU core, which is exactly the kind of thing that would eventually let this kernel run more than one hardware thread at once — a different, harder problem than the cooperative multitasking from Stage 6, and one this now has real footing for.

## Next

The two large remaining milestones — preemptive scheduling and a real disk driver — stay queued for slower, multi-session work.
