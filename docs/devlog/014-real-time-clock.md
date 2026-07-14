# Devlog 014 — Reading the real-time clock

**Milestone:** the OS can tell you the actual date and time. `date` prints the live wall clock, and running it twice shows the seconds advancing.

## Where the time comes from

Every PC has a small **CMOS** chip with its own battery, so it keeps counting the date and time even while the machine is powered off. That's how your computer knows the time the instant it boots, before touching the network. This stage reads that chip.

You talk to it through two I/O ports, a classic hardware pattern:

- write the number of the register you want to `0x70` (the "address" port),
- then read that register's value from `0x71` (the "data" port).

The clock fields live in fixed registers: seconds in `0x00`, minutes in `0x02`, hours in `0x04`, day/month/year in `0x07`/`0x08`/`0x09`. Read them, assemble a date. Simple in outline — but there are two traps.

## Trap 1: the numbers are in BCD

By default the RTC stores each value as **Binary-Coded Decimal**: each byte holds two decimal digits, one per nibble. So the byte for "59 seconds" is `0x59`, not `0x3B` (which is 59 in plain binary). Read it as binary and you'd get 89. The fix is a two-line decode — `(v & 0x0F) + (v >> 4) * 10` — but you have to know to do it. (Status Register B tells you whether the chip is in BCD or binary mode, and whether hours are 12- or 24-hour; the code checks both.)

## Trap 2: the clock ticks while you read it

The RTC updates once a second, and a read that lands *during* an update can return a half-updated, nonsensical value — imagine reading the minutes right as `12:59:59` rolls to `13:00:00` and catching the minutes before the hours. Two safeguards handle this:

1. Status Register A has an **"update in progress"** bit. Wait for it to clear before reading.
2. Read the whole set of fields, then read them **again**, and only trust the result when two consecutive reads match. If they differ, a tick happened mid-read, so try once more.

That "read until stable" loop is a small but real example of dealing with concurrent hardware: the device is changing state independently of your code, so you sample it defensively.

## Proof

The demo runs `date` twice a few seconds apart. The output — `2026-07-14 12:29:10 UTC` then `12:29:14 UTC` — shows two things at once: the date decoded correctly (so BCD handling is right), and the clock is genuinely *live* (the seconds moved). A wrong BCD decode would print gibberish like `2032` or `:71:`; a static read would show the same time twice.

## Why it's satisfying

It's the first time the kernel reaches out and reads a piece of real hardware state that exists entirely outside itself — not memory it manages, not an interrupt it configured, but a physical clock ticking on its own. "How does a computer know the time at boot?" has a concrete answer sitting in this file.

## Next

The two large remaining milestones — preemptive scheduling and a real disk driver — stay queued for slower, multi-session work.
