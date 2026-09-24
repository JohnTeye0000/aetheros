# Day 29 — The SYSCALL / SYSRET fast path

`int 0x80` works, but it's the slow, legacy way in: the CPU walks the IDT, checks gate privileges, switches stacks via the TSS and pushes a five-word frame. x86-64 has a dedicated pair of instructions, `syscall` and `sysretq`, that skip almost all of that. Both entry paths now exist side by side.

## Configuring it (three MSRs)

- **EFER.SCE** (bit 0) enables the instructions.
- **STAR**: bits 47:32 = kernel CS (SS is that + 8); bits 63:48 = the SYSRET base — SYSRET loads **SS = base+8, CS = base+16** with RPL forced to 3.
- **LSTAR** = where `syscall` jumps. **SFMASK** = RFLAGS bits to clear on entry (IF and DF).

Those "+8 / +16" rules dictate the GDT: kernel code (0x08), kernel data (0x10), **user data (0x18), user code (0x20)**, then the TSS. I reordered the GDT and added a kernel data segment for this; the base is computed from the user selectors rather than hard-coded.

## What `syscall` does NOT do

It doesn't switch stacks or save anything: the CPU puts the return RIP in `rcx`, RFLAGS in `r11`, and runs the handler **still on the user stack**. So `syscall_fast_entry` must, with interrupts off (SFMASK cleared IF):
1. stash the user `rsp`, load the kernel stack (a mirror of TSS `rsp0`, written by `enter_user`), align it;
2. save user rsp/rip/rflags and caller-saved registers, call the same `syscall_dispatch`;
3. restore, put the user rsp back, `sysretq`.

Consequence for programs: `syscall` clobbers `rcx` and `r11` (same as Linux). Arguments use the same convention as `int 0x80` (rax, rdi, rsi, rdx, r10).

## Verified

- `run fast`: greeting, sleep, getpid (status 1) and exit — all via `syscall`.
- `int 0x80` programs (`hello`), faults (`poke`), everything else unchanged.
- Benchmark programs `bint` / `bsys`, 200,000 getpid calls each, exit status = timer ticks: **96 vs 88** (~8% faster). Honest caveat: this runs under QEMU's software emulation at 18 Hz timer resolution, and the syscall body (scheduler lock, dispatch) dominates — it says the fast path isn't slower, not how big the real-hardware gap is.

## Limits

No `SMEP/SMAP`, one kernel-stack mirror (one user process at a time), interrupts stay off for the whole syscall.
