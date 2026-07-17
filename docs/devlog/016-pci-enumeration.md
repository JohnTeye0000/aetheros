# Devlog 016 — PCI bus enumeration (`lspci`)

**Milestone:** the kernel can discover what hardware is actually attached to the machine. `lspci` walks the PCI bus and lists every device it finds — by vendor/device ID, class, and (for the ones I recognize) a name.

## How an OS finds out what's there

Before a kernel can write a disk driver or a network driver, it needs to know a disk or a network card *exists* — and PCI is the mechanism that answers that. Every PCI device exposes a small block of **configuration space**: standardized registers holding its vendor ID, device ID, class, and other identifying facts, all readable over two I/O ports with no driver required:

- write a **bus/device/function/offset** packed into a 32-bit word to the **address port** (`0xCF8`),
- read the answer back from the **data port** (`0xCFC`).

That's the entire protocol. `config_read32` builds the address word per the PCI spec — bit 31 enables the access, then the bus (8 bits), device (5 bits), function (3 bits), and a 4-byte-aligned register offset — and reads back whatever's at that address.

## Probing an empty slot

Most bus/device combinations have nothing plugged in. How do you tell "no device here" from "device with vendor ID 0"? The bus itself answers: with nothing pulling the data lines low, a read from an absent slot comes back **all ones** — vendor ID `0xFFFF`, which is reserved and never a real vendor. So `probe()` just checks for that sentinel and treats it as "empty." Scanning is then a brute-force loop: try every device 0-31 on the bus, and for anything present, check the header-type byte's top bit to see whether it's *multi-function* before bothering to probe functions 1-7.

## A deliberate simplification, made honest

A truly thorough scanner follows PCI-to-PCI **bridges** to reach buses beyond 0 — a bridge's own config space tells you which bus numbers live behind it, so discovering the whole tree means recursing through bridges. That's a real piece of extra work, and I skipped it: this scanner only walks bus 0. On the PC platform (and every stock QEMU machine), the host bridge, the ISA/IDE/ACPI bridges, and the display all sit right there on bus 0, so the simplification doesn't cost us anything on this hardware — but it's worth being honest that "enumerate every bus by following bridges" is the natural next step, not something this already does.

## Real hardware, real surprises

The best proof this works is what showed up that I *didn't* code for. My known-device table only had five entries, but the scan found six devices — the sixth, `8086:100e`, wasn't in the table and printed as a bare vendor/device ID with class `network`. That's exactly what a good fallback should do: correctly identify and classify hardware it doesn't have a name for, rather than crashing or silently dropping it. (It turned out to be an Intel e1000 NIC, which QEMU adds to its default machine — I added the name afterward, but the *scanner* had already done its job before I knew what it would find.)

## Next

The two large remaining milestones — preemptive scheduling and a real disk driver — are still queued for slower, multi-session work. A disk driver, when it comes, will start from exactly this: using `lspci`'s output to find the IDE/AHCI controller to talk to.
