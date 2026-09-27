# Day 32 — Talking between processes: named channels

Stage 31 gave processes their own address spaces; today they get a sanctioned way to cross that boundary — named byte channels, with two new syscalls.

## The syscalls

| rax | name | args | blocks... |
|---|---|---|---|
| 9 | send | name ptr/len, data ptr/len | ...while the channel is full |
| 10 | recv | name ptr/len, buf ptr/len | ...while the channel is empty |

A channel is created lazily on first use and keeps its name, so a consumer started before its producer (or vice versa) still finds the same channel. Both calls **sleep** rather than spin while blocked — the same `sleep_ticks(1)` retry loop `readkey` already used — so a waiting process costs no CPU and the scheduler runs everything else.

`send` copies the whole message out of user memory up front (audited via `copy_from_user`), then feeds the channel's ring buffer as room frees up, so a message bigger than the channel's capacity still completes correctly. `recv` returns whatever is queued (up to the caller's buffer), at least one byte.

## Demo: producer / consumer, genuinely concurrent

`run producer` sends 5 tagged messages 6 ticks apart; `run consumer` receives 5. Started in either order, `ps` shows both alive at once — this is the point of Stage 31's per-process address spaces: two independent programs, two independent stacks, sharing state only through the one channel they both named:

```
run consumer
run producer
consumer got:  p1
producer sent: p1
producer sent: p2
consumer got:  p2
...
```

(Each program also echoes its own side locally via `write`, so the log shows both perspectives interleaved.)

## Notes

- Blocking via sleep means a channel operation's "does it ever return" question is answered the same way threads already answer it: check with `ps`, `kill` if stuck.
- Capacity is fixed (1 KiB) and channels are never explicitly destroyed (up to 8 total) — good enough for point-to-point pipes between a couple of processes, not a real IPC subsystem.
- No permission model: any process can send/recv on any name it knows.

## Limits

Byte-stream only (no message framing beyond what the programs agree on), no select/poll (one channel per blocking call), no back-pressure signalling beyond "still full/still empty".
