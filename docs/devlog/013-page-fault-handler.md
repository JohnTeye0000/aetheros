# Devlog 013 — A page-fault handler

**Milestone:** the kernel now handles page faults gracefully — reporting *what* address was touched and *how* — instead of letting a bad memory access spiral into a double fault.

## The most common memory bug gets a first-class report

A **page fault** (`#PF`, vector 14) is what the CPU raises when it can't translate a virtual address: the page is unmapped, or the access broke the page's permissions (writing a read-only page, running no-execute memory, user code touching kernel memory). It is *the* everyday memory error — a null-pointer dereference, a use-after-free, a stack overflow all surface as page faults.

Until now we had no handler for it, which meant any page fault escalated straight to a double fault. This stage gives it a proper handler, and the interesting part is how much useful information the hardware hands you if you know where to look.

## Two clues the CPU leaves behind

When a page fault fires, the CPU sets up two things for the handler:

1. **`CR2`** — a control register holding the exact **virtual address** that faulted. Reading it tells you *where* the program went wrong. Our demo writes to `0xdeadbeef`, and the handler prints exactly that back.

2. **An error code** — a bitfield describing the access. Our handler prints it decoded, and the demo shows `CAUSED_BY_WRITE`, correctly identifying that the faulting access was a *write*, not a read. The same code also distinguishes a missing page from a protection violation, and a user-mode access from a kernel one. That's the difference between "something's wrong with memory" and "a *write* to `0xdeadbeef` from kernel mode faulted because the page isn't present" — the second is debuggable.

## One signature detail

The page-fault handler's type is `fn(InterruptStackFrame, PageFaultErrorCode)` — it takes the error code as a real typed argument, not a raw `u64`. And unlike the double-fault handler, its return type is `()`, not `!`: page faults *are* recoverable in a full OS (that's exactly how demand paging and copy-on-write work — you fault on a page, the handler maps it in, and execution resumes). We can't do anything useful with this fault, though, so instead of returning (which would just re-run the faulting instruction and fault again forever), the handler prints the diagnosis and parks the CPU in a halt loop.

## Where this fits

Together with Stage 12's double-fault work, the kernel now has real, informative fault handling: it can tell you precisely why it died instead of silently rebooting or freezing. That diagnostic quality is unglamorous but it's exactly what makes a kernel *debuggable* — and "walk me through what happens on a page fault, and what `CR2` is for" is a textbook systems question with a working answer sitting right here.

## Next

The heavyweight remaining milestones — preemptive scheduling and a real disk driver — are still queued for slower, multi-session work.
