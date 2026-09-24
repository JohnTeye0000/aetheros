# Day 30 — NX, SMEP and SMAP: making bugs harder to exploit

Ring separation stops a *well-behaved* boundary from being crossed. Hardening is about the day the kernel (or a program) has a bug. Three CPU features, turned on and verified:

| feature | switch | stops |
|---|---|---|
| **NX** | EFER.NXE + `NO_EXECUTE` page flag | running data as code (the user stack is now W^X) |
| **SMEP** | CR4 bit 20 | the *kernel* executing a user-supplied page |
| **SMAP** | CR4 bit 21 | the *kernel* reading/writing user memory by accident |

SMEP/SMAP only exist on newer CPUs, so `harden()` reads cpuid leaf 7 and enables only what's reported, printing `[harden] NX on  SMEP on  SMAP on` (or `unsupported`). QEMU's default CPU has neither; `-cpu max` has both.

## SMAP forced an audit

With SMAP on, any kernel touch of a user page faults unless RFLAGS.AC is set (`stac` … `clac`). So all kernel/user copying now goes through exactly two functions, `copy_from_user` and `copy_to_user`, which bracket the copy with stac/clac. Everything else works on kernel-side copies (`Vec`/`String`). Previously syscalls held raw slices pointing into user memory across allocations and file lookups — fine functionally, but the kind of pattern SMAP exists to catch. Interrupts are off during syscalls, so AC can't leak to another context.

## Demos

- `run nx` writes a `ret` byte onto its stack and jumps to it → `page fault: instruction fetch from a no-execute page (NX)`, killed.
- `run selfmod` overwrites its own code → `page fault: write to a read-only page`, killed.
- Page-fault reporting now decodes the error code (instruction fetch / read-only write) instead of a generic message.

## Verified

`-cpu max`: all three on; `cat`, `note`, `keys`, `fast`, `hello` all still work (a missed stac would panic the kernel — none did), `nx` and `selfmod` killed. Default CPU: NX on, SMEP/SMAP reported unsupported, same results.

## Limits

I could not demonstrate a SMEP/SMAP *trip* — that needs an actual kernel bug to exploit, and I'd rather not write one on purpose. What is shown is that the protected configuration runs the whole syscall surface without faults.
