# Day 26 — Ring 3: user mode and system calls

Until today every line of code ran at ring 0 with total control. Now the kernel can run a program at **ring 3**, where the CPU itself stops it from touching the kernel — and the kernel survives when the program misbehaves.

## What had to exist

1. **GDT**: two DPL-3 segments (user code, user data). Selectors carry RPL 3.
2. **TSS `rsp0`**: when an interrupt arrives in ring 3 the CPU must switch to a kernel stack; it reads it from the TSS. The TSS became a `static mut` so `rsp0` can be re-pointed for each run.
3. **User pages**: frames mapped with `USER_ACCESSIBLE` at *every* level of the page-table walk (leaf **and** parent tables), in a top-level slot found unused at runtime so we never edit tables the kernel relies on. Code is read-only; stack is writable. Kernel memory has no user bit, so ring 3 can't see it.
4. **Entering ring 3**: `enter_user` pushes SS, RSP, RFLAGS, CS, RIP and runs `iretq` — a "return" to somewhere the CPU never was. RFLAGS has IF set, so the timer keeps preempting user code; IOPL 0, so no port I/O.
5. **Syscalls**: `int 0x80` via an IDT gate with DPL 3 (other vectors raise #GP for user code). `syscall_entry` (asm) saves caller-saved registers, calls `syscall_dispatch`, and `iretq`s with the result in rax.
6. **Leaving**: `enter_user` saves the kernel rsp; `exit` (or a fault) calls `kernel_resume`, which restores it and `ret`s — from the launching thread's view `enter_user` simply returned.

## Syscalls

`exit(0)`, `write(1)`, `ticks(2)`, `getpid(3)`, `sleep(4)`. `write` validates the buffer lies entirely inside the program's own mapped code/stack pages (not the unmapped gap between them — a kernel fault there would be fatal) and prints only ASCII.

## Built-in programs (`run <name>`)

| name | shows |
|---|---|
| `hello` | write + exit |
| `count` | write + sleep syscall (blocks, no CPU) |
| `spin` | 400M-iteration loop in ring 3; `ps` and the shell stay responsive — the timer preempts user code |
| `hlt` | privileged instruction → #GP → program killed, kernel fine |
| `poke` | writes to the VGA buffer → page fault (`PROTECTION_VIOLATION | CAUSED_BY_WRITE | USER_MODE`) → killed |
| `badptr` | passes a kernel address to `write` → returns -1 |

Exception handlers check the saved CS: RPL 3 means "user's fault" → kill the program; RPL 0 still means a kernel bug → panic as before.

## Bugs / notes

- LLVM's Intel syntax reads `mov esi, end - start` as a memory operand; fixed with `.equ len, end - start` and `mov esi, offset len`.
- A first patch to the page-fault handler silently didn't apply (mixed CRLF/LF line endings broke a multi-line string match) — the `poke` test caught it because the kernel printed the old fatal page-fault screen. Tests catch what patch scripts miss.

## Limits (stated plainly)

One user process at a time; programs are embedded in the kernel image (Stage 27 loads them from the filesystem); no per-process address space (they share the kernel's tables, just with user-visible pages in one slot); user pages are reused, not freed; `int 0x80` is slower than `syscall/sysret`; no SMAP/SMEP.
