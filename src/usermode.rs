//! User mode (ring 3) and system calls.
//!
//! Until now everything ran in ring 0 with full power over the machine. This
//! module runs code at the **lowest privilege level**, where the CPU itself
//! enforces the boundary:
//!
//! * user code can only touch pages whose page-table entries say
//!   `USER_ACCESSIBLE` (the whole kernel is invisible to it);
//! * privileged instructions (`hlt`, `cli`, `in`/`out`, ...) raise a
//!   general-protection fault instead of executing;
//! * the only way *in* to the kernel is through a gate we chose: `int 0x80`.
//!
//! ## How a program runs
//!
//! 1. The program's bytes are copied into a few physical frames mapped (user +
//!    read-only) at `BASE`; two more frames are mapped writable as its stack.
//! 2. A kernel thread calls `enter_user`, which pushes the five words `iretq`
//!    expects (SS, RSP, RFLAGS, CS, RIP) and executes `iretq` — the CPU "returns"
//!    to an address in ring 3 that it never came from.
//! 3. The program talks to the kernel with `int 0x80` (rax = syscall number).
//!    The CPU switches to the kernel stack named in the TSS (`rsp0`), and
//!    `syscall_entry` saves registers and calls `syscall_dispatch`.
//! 4. `exit` (or a fault) restores the kernel stack pointer saved by
//!    `enter_user` and *returns from it*, so from the thread's point of view
//!    `enter_user` is just a function that came back.
//!
//! ## System calls
//!
//! | rax | name  | args                    | returns |
//! |-----|-------|-------------------------|---------|
//! | 0   | exit  | rdi = status            | never   |
//! | 1   | write | rdi = ptr, rsi = len    | len, or -1 if the pointer is bad |
//! | 2   | ticks | -                       | timer ticks since boot |
//! | 3   | getpid| -                       | thread id |
//! | 4   | sleep | rdi = ticks (max 100)   | 0 |
//!
//! Every pointer a user program hands us is **validated** before the kernel
//! dereferences it: it must lie entirely inside the program's own mapped code
//! or stack pages. A user program must never be able to make the kernel read
//! (or print!) kernel memory on its behalf.

use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

use crate::{print, serial_print};

const CODE_PAGES: usize = 4;
const STACK_PAGES: usize = 2;
const CODE_BYTES: u64 = (CODE_PAGES * 4096) as u64;
const STACK_BYTES: u64 = (STACK_PAGES * 4096) as u64;
/// Stack sits well above the code, with an unmapped gap between them.
const STACK_OFFSET: u64 = 0x10000;

/// Returned by `enter_user` when the program was killed by a fault rather than
/// calling `exit` (exit statuses are masked to 32 bits, so they can't collide).
const FAULT_STATUS: u64 = u64::MAX;

const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_TICKS: u64 = 2;
const SYS_GETPID: u64 = 3;
const SYS_SLEEP: u64 = 4;
const SYS_READFILE: u64 = 5;
const SYS_GETARGS: u64 = 6;
const SYS_READKEY: u64 = 7;
const SYS_WRITEFILE: u64 = 8;
const SYS_SEND: u64 = 9;
const SYS_RECV: u64 = 10;
const SYS_SPAWN: u64 = 11;

global_asm!(
    r#"
    // enter_user(entry rdi, user_rsp rsi, saved_slot rdx, cs rcx, ss r8, rsp0_slot r9) -> status
    .global enter_user
    .global fast_kernel_rsp
    enter_user:
        push rbp
        push rbx
        push r12
        push r13
        push r14
        push r15
        mov  [rdx], rsp          // where `exit` will come back to
        mov  [r9], rsp           // ring 3 -> ring 0 interrupts land just below this
        mov  [rip + fast_kernel_rsp], rsp   // ...and so does the SYSCALL path
        push r8                  // SS
        push rsi                 // RSP
        push 0x202               // RFLAGS: IF set (timer keeps preempting), IOPL 0
        push rcx                 // CS
        push rdi                 // RIP
        xor  eax, eax            // don't leak kernel register contents to ring 3
        xor  ebx, ebx
        xor  ecx, ecx
        xor  edx, edx
        xor  esi, esi
        xor  edi, edi
        xor  ebp, ebp
        xor  r8d, r8d
        xor  r9d, r9d
        xor  r10d, r10d
        xor  r11d, r11d
        xor  r12d, r12d
        xor  r13d, r13d
        xor  r14d, r14d
        xor  r15d, r15d
        iretq

    // kernel_resume(saved_rsp rdi, status rsi): make `enter_user` return `status`.
    .global kernel_resume
    kernel_resume:
        mov  rsp, rdi
        mov  rax, rsi
        pop  r15
        pop  r14
        pop  r13
        pop  r12
        pop  rbx
        pop  rbp
        ret

    // int 0x80 lands here on the kernel stack. The CPU has pushed SS RSP RFLAGS
    // CS RIP (rsp is 8 mod 16); we save every register the C ABI may clobber.
    .global syscall_entry
    syscall_entry:
        push rcx
        push rdx
        push rsi
        push rdi
        push r8
        push r9
        push r10
        push r11
        mov  rcx, rdx            // arg3
        mov  rdx, rsi            // arg2
        mov  rsi, rdi            // arg1
        mov  r8, r10             // arg4
        mov  rdi, rax            // syscall number
        sub  rsp, 8              // 16-byte align for the call
        cld
        call syscall_dispatch
        add  rsp, 8
        pop  r11
        pop  r10
        pop  r9
        pop  r8
        pop  rdi
        pop  rsi
        pop  rdx
        pop  rcx
        iretq                    // rax = return value

    // ---- SYSCALL fast path -------------------------------------------------
    // `syscall` (unlike int 0x80) does NOT switch stacks or push a frame: the
    // CPU just puts the return RIP in rcx, RFLAGS in r11, clears the flags in
    // SFMASK (IF), loads CS/SS from STAR, and jumps to LSTAR. Everything else
    // is on us -- notably we are still on the USER stack, so the first job is
    // to get onto the kernel stack.
    .data
    .balign 8
    fast_kernel_rsp: .quad 0
    fast_scratch:    .quad 0
    .text

    .global syscall_fast_entry
    syscall_fast_entry:
        mov  [rip + fast_scratch], rsp      // interrupts are off: nothing can intervene
        mov  rsp, [rip + fast_kernel_rsp]
        and  rsp, -16
        push qword ptr [rip + fast_scratch]         // user rsp
        push rcx                            // user rip
        push r11                            // user rflags
        push rdx
        push rsi
        push rdi
        push r8
        push r9
        push r10
        mov  rcx, rdx
        mov  rdx, rsi
        mov  rsi, rdi
        mov  r8, r10
        mov  rdi, rax
        sub  rsp, 8                         // 9 pushes -> re-align for the call
        cld
        call syscall_dispatch
        add  rsp, 8
        pop  r10
        pop  r9
        pop  r8
        pop  rdi
        pop  rsi
        pop  rdx
        pop  r11                            // rflags -> restored by sysret
        pop  rcx                            // rip    -> restored by sysret
        pop  rsp                            // back onto the user stack
        sysretq

    // ---- built-in user programs (position independent, ring 3) ----------

    .section .rodata
    .global user_hello_start
    .global user_hello_end
    user_hello_start:
        mov  eax, 1
        lea  rdi, [rip + uh_msg]
        mov  esi, offset uh_msg_len
        int  0x80
        mov  eax, 0
        xor  edi, edi
        int  0x80
    uh_msg:
        .ascii "Hello from ring 3! I am an unprivileged program talking to the kernel via int 0x80.\n"
    uh_msg_end:
    .equ uh_msg_len, uh_msg_end - uh_msg
    user_hello_end:

    .global user_count_start
    .global user_count_end
    user_count_start:
        sub  rsp, 16
        mov  r12d, 1
    uc_loop:
        mov  byte ptr [rsp], 0x23        // '#'
        mov  eax, r12d
        add  eax, 0x30
        mov  byte ptr [rsp + 1], al      // digit
        mov  byte ptr [rsp + 2], 0x0a
        mov  eax, 1
        mov  rdi, rsp
        mov  esi, 3
        int  0x80
        mov  eax, 4                      // sleep 9 ticks (~0.5 s)
        mov  edi, 9
        int  0x80
        inc  r12d
        cmp  r12d, 5
        jle  uc_loop
        mov  eax, 0
        xor  edi, edi
        int  0x80
    user_count_end:

    .global user_spin_start
    .global user_spin_end
    user_spin_start:
        mov  eax, 1
        lea  rdi, [rip + us_msg1]
        mov  esi, offset us_msg1_len
        int  0x80
        mov  rcx, 400000000              // burn CPU in ring 3; the timer preempts us
    us_loop:
        dec  rcx
        jnz  us_loop
        mov  eax, 1
        lea  rdi, [rip + us_msg2]
        mov  esi, offset us_msg2_len
        int  0x80
        mov  eax, 0
        xor  edi, edi
        int  0x80
    us_msg1:
        .ascii "spinning in ring 3 (try ps / typing - the timer still preempts me)...\n"
    us_msg1_end:
    .equ us_msg1_len, us_msg1_end - us_msg1
    us_msg2:
        .ascii "spin finished.\n"
    us_msg2_end:
    .equ us_msg2_len, us_msg2_end - us_msg2
    user_spin_end:

    .global user_hlt_start
    .global user_hlt_end
    user_hlt_start:
        mov  eax, 1
        lea  rdi, [rip + uhl_msg]
        mov  esi, offset uhl_msg_len
        int  0x80
        hlt                               // privileged: must raise #GP in ring 3
        mov  eax, 0
        xor  edi, edi
        int  0x80
    uhl_msg:
        .ascii "about to execute HLT (a privileged instruction) in ring 3...\n"
    uhl_msg_end:
    .equ uhl_msg_len, uhl_msg_end - uhl_msg
    user_hlt_end:

    .global user_poke_start
    .global user_poke_end
    user_poke_start:
        mov  eax, 1
        lea  rdi, [rip + up_msg]
        mov  esi, offset up_msg_len
        int  0x80
        mov  eax, 0xb8000                 // the VGA buffer: kernel-only memory
        mov  dword ptr [rax], 0x41414141  // must raise a page fault (protection)
        mov  eax, 0
        xor  edi, edi
        int  0x80
    up_msg:
        .ascii "about to write to kernel memory (the VGA buffer) from ring 3...\n"
    up_msg_end:
    .equ up_msg_len, up_msg_end - up_msg
    user_poke_end:

    .global user_badptr_start
    .global user_badptr_end
    user_badptr_start:
        mov  eax, 1                       // write(kernel address, 16)
        mov  edi, 0xb8000
        mov  esi, 16
        int  0x80
        cmp  rax, -1
        jne  ub_bug
        mov  eax, 1
        lea  rdi, [rip + ub_ok]
        mov  esi, offset ub_ok_len
        int  0x80
        mov  eax, 0
        xor  edi, edi
        int  0x80
    ub_bug:
        mov  eax, 1
        lea  rdi, [rip + ub_bad]
        mov  esi, offset ub_bad_len
        int  0x80
        mov  eax, 0
        mov  edi, 1
        int  0x80
    ub_ok:
        .ascii "kernel rejected my bad pointer (write returned -1). Good.\n"
    ub_ok_end:
    .equ ub_ok_len, ub_ok_end - ub_ok
    ub_bad:
        .ascii "BUG: kernel accepted a pointer into kernel memory!\n"
    ub_bad_end:
    .equ ub_bad_len, ub_bad_end - ub_bad
    user_badptr_end:

    .global user_cat_start
    .global user_cat_end
    user_cat_start:
        sub  rsp, 2048
        mov  eax, 6                      // getargs(rsp, 256) -> len
        mov  rdi, rsp
        mov  esi, 256
        int  0x80
        mov  r12, rax
        cmp  r12, 0
        jle  uk_usage
        mov  eax, 5                      // readfile(name, len, rsp+256, 1500)
        mov  rdi, rsp
        mov  rsi, r12
        lea  rdx, [rsp + 256]
        mov  r10d, 1500
        int  0x80
        cmp  rax, 0
        jl   uk_fail
        mov  rsi, rax                    // write(rsp+256, n)
        lea  rdi, [rsp + 256]
        mov  eax, 1
        int  0x80
        mov  eax, 0
        xor  edi, edi
        int  0x80
    uk_usage:
        mov  eax, 1
        lea  rdi, [rip + uk_m1]
        mov  esi, offset uk_m1_len
        int  0x80
        mov  eax, 0
        mov  edi, 2
        int  0x80
    uk_fail:
        mov  eax, 1
        lea  rdi, [rip + uk_m2]
        mov  esi, offset uk_m2_len
        int  0x80
        mov  eax, 0
        mov  edi, 1
        int  0x80
    uk_m1:
        .ascii "usage: run cat <filename>\n"
    uk_m1_end:
    .equ uk_m1_len, uk_m1_end - uk_m1
    uk_m2:
        .ascii "cat: could not read that file\n"
    uk_m2_end:
    .equ uk_m2_len, uk_m2_end - uk_m2
    user_cat_end:

    .global user_keys_start
    .global user_keys_end
    user_keys_start:
        sub  rsp, 16
        mov  eax, 1
        lea  rdi, [rip + ukk_m]
        mov  esi, offset ukk_m_len
        int  0x80
        xor  r12d, r12d
    ukk_loop:
        mov  eax, 7                      // readkey() blocks until a key arrives
        int  0x80
        cmp  eax, 10
        je   ukk_done
        cmp  eax, 13
        je   ukk_done
        mov  byte ptr [rsp], al
        inc  r12d
        mov  eax, 1                      // echo it
        mov  rdi, rsp
        mov  esi, 1
        int  0x80
        jmp  ukk_loop
    ukk_done:
        mov  byte ptr [rsp], 0x0a
        mov  eax, 1
        mov  rdi, rsp
        mov  esi, 1
        int  0x80
        mov  eax, 0                      // exit(chars typed)
        mov  edi, r12d
        int  0x80
    ukk_m:
        .ascii "typing into a ring-3 program (Enter to finish): "
    ukk_m_end:
    .equ ukk_m_len, ukk_m_end - ukk_m
    user_keys_end:

    .global user_note_start
    .global user_note_end
    user_note_start:
        mov  eax, 8                      // writefile("made.txt", 8, msg, len)
        lea  rdi, [rip + un_name]
        mov  esi, 8
        lea  rdx, [rip + un_msg]
        mov  r10d, offset un_msg_len
        int  0x80
        mov  edi, 0
        cmp  rax, -1
        jne  un_exit
        mov  edi, 1
    un_exit:
        mov  eax, 0
        int  0x80
    un_name:
        .ascii "made.txt"
    un_msg:
        .ascii "written by a ring-3 program via the writefile syscall\n"
    un_msg_end:
    .equ un_msg_len, un_msg_end - un_msg
    user_note_end:

    .global user_fast_start
    .global user_fast_end
    user_fast_start:
        mov  eax, 1
        lea  rdi, [rip + uf_msg]
        mov  esi, offset uf_msg_len
        syscall
        mov  eax, 4
        mov  edi, 2
        syscall
        mov  eax, 3                      // getpid via syscall
        syscall
        mov  edi, eax
        mov  eax, 0
        syscall
    uf_msg:
        .ascii "hello via the SYSCALL instruction (no interrupt gate, no frame push)\n"
    uf_msg_end:
    .equ uf_msg_len, uf_msg_end - uf_msg
    user_fast_end:

    .global user_bint_start
    .global user_bint_end
    user_bint_start:
        mov  eax, 2
        int  0x80
        mov  r13, rax
        mov  r12d, 200000
    ubi_loop:
        mov  eax, 3
        int  0x80
        dec  r12d
        jnz  ubi_loop
        mov  eax, 2
        int  0x80
        sub  rax, r13
        mov  rdi, rax
        mov  eax, 0
        int  0x80
    user_bint_end:

    .global user_bsys_start
    .global user_bsys_end
    user_bsys_start:
        mov  eax, 2
        int  0x80
        mov  r13, rax
        mov  r12d, 200000
    ubs_loop:
        mov  eax, 3
        syscall
        dec  r12d
        jnz  ubs_loop
        mov  eax, 2
        int  0x80
        sub  rax, r13
        mov  rdi, rax
        mov  eax, 0
        int  0x80
    user_bsys_end:

    .global user_nx_start
    .global user_nx_end
    user_nx_start:
        mov  eax, 1
        lea  rdi, [rip + unx_msg]
        mov  esi, offset unx_msg_len
        int  0x80
        sub  rsp, 16
        mov  byte ptr [rsp], 0xc3        // a `ret` instruction, as DATA on the stack
        jmp  rsp                         // ...now try to EXECUTE the stack
        mov  eax, 0
        xor  edi, edi
        int  0x80
    unx_msg:
        .ascii "writing code onto my stack and jumping to it (NX should stop me)...\n"
    unx_msg_end:
    .equ unx_msg_len, unx_msg_end - unx_msg
    user_nx_end:

    .global user_selfmod_start
    .global user_selfmod_end
    user_selfmod_start:
        mov  eax, 1
        lea  rdi, [rip + usm_msg]
        mov  esi, offset usm_msg_len
        int  0x80
        lea  rax, [rip + user_selfmod_start]
        mov  byte ptr [rax], 0x90        // overwrite my own code
        mov  eax, 0
        xor  edi, edi
        int  0x80
    usm_msg:
        .ascii "trying to overwrite my own code page (it is read-only)...\n"
    usm_msg_end:
    .equ usm_msg_len, usm_msg_end - usm_msg
    user_selfmod_end:

    .global user_iso_start
    .global user_iso_end
    user_iso_start:
        sub  rsp, 256
        mov  eax, 6                      // getargs(rsp, 32): my private tag
        mov  rdi, rsp
        mov  esi, 32
        int  0x80
        mov  r12, rax
        cmp  r12, 0
        jg   ui_have
        mov  r12d, 1
        mov  byte ptr [rsp], 0x3f        // '?'
    ui_have:
        mov  eax, 4                      // sleep: other processes run meanwhile,
        mov  edi, 20                     // storing THEIR tag at this same address
        int  0x80
        mov  eax, 1
        lea  rdi, [rip + ui_m]
        mov  esi, offset ui_m_len
        int  0x80
        mov  byte ptr [rsp + r12], 0x0a
        lea  rsi, [r12 + 1]
        mov  eax, 1
        mov  rdi, rsp
        int  0x80
        mov  eax, 0
        xor  edi, edi
        int  0x80
    ui_m:
        .ascii "memory at my stack address still holds: "
    ui_m_end:
    .equ ui_m_len, ui_m_end - ui_m
    user_iso_end:

    .global user_producer_start
    .global user_producer_end
    user_producer_start:
        sub  rsp, 32
        mov  r12d, 1
    upr_loop:
        mov  byte ptr [rsp], 0x70        // 'p'
        mov  eax, r12d
        add  eax, 0x30
        mov  byte ptr [rsp + 1], al
        mov  byte ptr [rsp + 2], 0x0a
        mov  eax, 9                      // send("chan", 4, rsp, 3)
        lea  rdi, [rip + upr_name]
        mov  esi, 4
        mov  rdx, rsp
        mov  r10d, 3
        int  0x80
        mov  eax, 1                      // also echo locally, so the log shows both sides
        lea  rdi, [rip + upr_tag]
        mov  esi, offset upr_tag_len
        int  0x80
        mov  eax, 1
        mov  rdi, rsp
        mov  esi, 3
        int  0x80
        mov  eax, 4
        mov  edi, 6
        int  0x80
        inc  r12d
        cmp  r12d, 5
        jle  upr_loop
        mov  eax, 0
        xor  edi, edi
        int  0x80
    upr_name:
        .ascii "chan"
    upr_tag:
        .ascii "producer sent: "
    upr_tag_end:
    .equ upr_tag_len, upr_tag_end - upr_tag
    user_producer_end:

    .global user_consumer_start
    .global user_consumer_end
    user_consumer_start:
        sub  rsp, 32
        mov  r12d, 1
    uco_loop:
        mov  eax, 10                     // recv("chan", 4, rsp, 32)
        lea  rdi, [rip + uco_name]
        mov  esi, 4
        mov  rdx, rsp
        mov  r10d, 32
        int  0x80
        mov  r13, rax                    // bytes received
        mov  eax, 1
        lea  rdi, [rip + uco_tag]
        mov  esi, offset uco_tag_len
        int  0x80
        mov  eax, 1
        mov  rdi, rsp
        mov  rsi, r13
        int  0x80
        inc  r12d
        cmp  r12d, 5
        jle  uco_loop
        mov  eax, 0
        xor  edi, edi
        int  0x80
    uco_name:
        .ascii "chan"
    uco_tag:
        .ascii "consumer got:  "
    uco_tag_end:
    .equ uco_tag_len, uco_tag_end - uco_tag
    user_consumer_end:

    .global user_launcher_start
    .global user_launcher_end
    user_launcher_start:
        mov  eax, 1
        lea  rdi, [rip + ul_msg]
        mov  esi, offset ul_msg_len
        int  0x80
        mov  eax, 11                     // spawn("hello", 5, 0, 0)
        lea  rdi, [rip + ul_hello]
        mov  esi, 5
        xor  edx, edx
        xor  r10d, r10d
        int  0x80
        mov  r12, rax
        mov  eax, 11                     // spawn("cat", 3, "motd", 4)
        lea  rdi, [rip + ul_cat]
        mov  esi, 3
        lea  rdx, [rip + ul_arg]
        mov  r10d, 4
        int  0x80
        mov  r13, rax
        mov  eax, 4                      // give the children time to run and print
        mov  edi, 20
        int  0x80
        mov  eax, 1
        lea  rdi, [rip + ul_done]
        mov  esi, offset ul_done_len
        int  0x80
        mov  eax, 0
        xor  edi, edi
        int  0x80
    ul_msg:
        .ascii "launcher: spawning two child programs via the spawn syscall...\n"
    ul_msg_end:
    .equ ul_msg_len, ul_msg_end - ul_msg
    ul_hello:
        .ascii "hello"
    ul_cat:
        .ascii "cat"
    ul_arg:
        .ascii "motd"
    ul_done:
        .ascii "launcher: done (children run independently - see ps while they're alive).\n"
    ul_done_end:
    .equ ul_done_len, ul_done_end - ul_done
    user_launcher_end:

    .text
    "#
);

extern "C" {
    fn enter_user(
        entry: u64,
        user_rsp: u64,
        saved_slot: *mut u64,
        cs: u64,
        ss: u64,
        rsp0_slot: *mut u64,
    ) -> u64;
    fn kernel_resume(saved_rsp: u64, status: u64) -> !;
    fn syscall_entry();
    fn syscall_fast_entry();
    static mut fast_kernel_rsp: u64;
}

/// Address of the `int 0x80` entry stub, for the IDT.
pub fn syscall_entry_addr() -> u64 {
    syscall_entry as *const () as usize as u64
}

/// Turn on the CPU's memory-protection features, and report what we got.
///
/// * **NX** (EFER.NXE): pages can be marked no-execute, so data (the user
///   stack) can't be run as code -- classic stack-smashing shellcode dies here.
/// * **SMEP** (CR4.SMEP): the kernel faults if it ever *executes* a user page,
///   so a kernel bug can't be steered into running user-supplied code.
/// * **SMAP** (CR4.SMAP): the kernel faults if it *reads/writes* user pages
///   without an explicit `stac`, so a kernel bug can't be tricked into
///   dereferencing a user pointer unchecked.
///
/// SMEP/SMAP are only present on newer CPUs (cpuid leaf 7), so they're enabled
/// only if reported. Returns (nx, smep, smap).
pub fn harden() -> (bool, bool, bool) {
    use core::arch::x86_64::__cpuid_count;
    use x86_64::registers::control::{Cr4, Cr4Flags};
    use x86_64::registers::model_specific::Msr;

    let mut efer = Msr::new(0xC000_0080);
    unsafe { efer.write(efer.read() | (1 << 11)) };

    let leaf7 = unsafe { __cpuid_count(7, 0) };
    let smep = leaf7.ebx & (1 << 7) != 0;
    let smap = leaf7.ebx & (1 << 20) != 0;
    unsafe {
        Cr4::update(|f| {
            if smep {
                f.insert(Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION);
            }
            if smap {
                f.insert(Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION);
            }
        });
    }
    SMAP_ON.store(smap, Ordering::SeqCst);
    (true, smep, smap)
}

/// Enable and configure the SYSCALL/SYSRET instructions.
///
/// * EFER.SCE turns the instructions on.
/// * STAR[47:32] = kernel CS (SS = that + 8); STAR[63:48] = base for SYSRET,
///   which loads SS = base + 8 and CS = base + 16 (RPL forced to 3). Our GDT
///   is laid out to make those arithmetic rules come out right.
/// * LSTAR = where `syscall` jumps. SFMASK = RFLAGS bits to clear on entry
///   (IF, so no interrupt can arrive while we're still on the user stack; DF).
fn init_syscall_msrs() {
    use x86_64::registers::model_specific::Msr;
    let (kcs, _kds) = crate::gdt::kernel_selectors();
    let (ucs, _uss) = crate::gdt::user_selectors();
    // ucs = base + 16 (with RPL 3 bits), so base = ucs - 16 - 3.
    let sysret_base = (ucs as u64 & !3) - 16;
    unsafe {
        let mut efer = Msr::new(0xC000_0080);
        let v = efer.read();
        efer.write(v | 1); // SCE
        Msr::new(0xC000_0081).write((sysret_base << 48) | ((kcs as u64) << 32));
        Msr::new(0xC000_0082).write(syscall_fast_entry as *const () as usize as u64);
        Msr::new(0xC000_0084).write(0x200 | 0x400);
    }
}

// --- the built-in programs -------------------------------------------------

macro_rules! blob {
    ($start:ident, $end:ident) => {{
        extern "C" {
            static $start: u8;
            static $end: u8;
        }
        unsafe {
            let a = core::ptr::addr_of!($start) as usize;
            let b = core::ptr::addr_of!($end) as usize;
            core::slice::from_raw_parts(a as *const u8, b - a)
        }
    }};
}

/// `(name, one-line description)` of every built-in program.
pub const PROGRAMS: &[(&str, &str)] = &[
    ("hello", "write a greeting via syscall, then exit"),
    ("count", "print #1..#5 with a 9-tick sleep syscall between"),
    ("spin", "burn CPU in ring 3 while the timer preempts it"),
    ("hlt", "execute a privileged instruction -> #GP -> killed"),
    ("poke", "write to kernel memory -> page fault -> killed"),
    ("badptr", "pass the kernel a kernel pointer -> write() refuses"),
    ("cat", "read the file named in argv via readfile, print it"),
    ("keys", "read keystrokes through the readkey syscall"),
    ("note", "create made.txt from ring 3 via writefile"),
    ("fast", "greet, sleep, getpid and exit via the SYSCALL instruction"),
    ("iso", "store <arg> on the stack, sleep, read it back (isolation demo)"),
    ("producer", "send 5 messages on channel 'chan' via the send syscall"),
    ("consumer", "receive 5 messages from channel 'chan' via recv"),
    ("launcher", "spawn other programs (hello, cat motd) via the spawn syscall"),
    ("nx", "execute code from the stack -> NX page fault -> killed"),
    ("selfmod", "overwrite own code page -> W^X page fault -> killed"),
    ("bint", "200k getpid calls via int 0x80; exit status = ticks taken"),
    ("bsys", "200k getpid calls via SYSCALL; exit status = ticks taken"),
];

pub fn builtin(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "hello" => blob!(user_hello_start, user_hello_end),
        "count" => blob!(user_count_start, user_count_end),
        "spin" => blob!(user_spin_start, user_spin_end),
        "hlt" => blob!(user_hlt_start, user_hlt_end),
        "poke" => blob!(user_poke_start, user_poke_end),
        "badptr" => blob!(user_badptr_start, user_badptr_end),
        "cat" => blob!(user_cat_start, user_cat_end),
        "keys" => blob!(user_keys_start, user_keys_end),
        "note" => blob!(user_note_start, user_note_end),
        "fast" => blob!(user_fast_start, user_fast_end),
        "iso" => blob!(user_iso_start, user_iso_end),
        "producer" => blob!(user_producer_start, user_producer_end),
        "consumer" => blob!(user_consumer_start, user_consumer_end),
        "launcher" => blob!(user_launcher_start, user_launcher_end),
        "nx" => blob!(user_nx_start, user_nx_end),
        "selfmod" => blob!(user_selfmod_start, user_selfmod_end),
        "bint" => blob!(user_bint_start, user_bint_end),
        "bsys" => blob!(user_bsys_start, user_bsys_end),
        _ => return None,
    })
}

// --- per-process address spaces -----------------------------------------------
//
// Every user process gets its OWN top-level page table (PML4). Two processes
// both see their code at `USER_BASE` and their stack just above it -- the same
// virtual addresses -- yet those addresses map to different physical frames, so
// neither can see (or corrupt) the other. The kernel's own mappings are copied
// into every table (the entries point at the same lower-level tables, so the
// kernel is shared and stays at the same addresses in every address space).
// The scheduler switches CR3 whenever it switches threads (`on_switch_to`).

/// Virtual address where every process's code is mapped (one PML4 slot chosen
/// once at first use: a slot that is empty in the kernel's own tables).
static USER_BASE: AtomicU64 = AtomicU64::new(0);

const MAX_PROCS: usize = 4;

struct Proc {
    tid: usize,
    name: String,
    code_len: usize,
    pml4: PhysFrame,
    /// Every physical frame this process owns (PML4, page tables, code, stack),
    /// so teardown can hand them all back.
    frames: Vec<PhysFrame>,
    /// Kernel stack pointer saved by `enter_user`: where `exit`/faults resume.
    saved_rsp: u64,
    args: String,
    fault_kind: usize,
    fault_addr: u64,
    fault_rip: u64,
}

/// All live processes. Only ever locked with interrupts disabled.
static PROCS: Mutex<Vec<Box<Proc>>> = Mutex::new(Vec::new());

fn with_current<R>(f: impl FnOnce(&mut Proc) -> R) -> Option<R> {
    let tid = crate::scheduler::current_id();
    x86_64::instructions::interrupts::without_interrupts(|| {
        PROCS.lock().iter_mut().find(|p| p.tid == tid).map(|p| f(p))
    })
}

/// A frame allocator wrapper that remembers everything it hands out, so the
/// process can later give every frame back.
struct Track<'a> {
    inner: &'a mut crate::memory::BootInfoFrameAllocator,
    got: Vec<PhysFrame>,
}

unsafe impl FrameAllocator<Size4KiB> for Track<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let f = self.inner.allocate_frame()?;
        self.got.push(f);
        Some(f)
    }
}

fn map_one(
    mapper: &mut OffsetPageTable,
    track: &mut Track,
    addr: u64,
    frame: PhysFrame,
    flags: PageTableFlags,
) -> Result<(), &'static str> {
    // Parent tables must be user-accessible too, or the leaf's user bit is moot.
    let table = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(addr));
    unsafe {
        mapper
            .map_to_with_table_flags(page, frame, flags, table, track)
            .map_err(|_| "mapping a user page failed")?
            .ignore(); // not the active tables: nothing to flush
    }
    Ok(())
}

/// Zero-fill / access a physical frame through the kernel's physical-memory map.
fn frame_bytes(frame: PhysFrame) -> &'static mut [u8] {
    let v = crate::memory::phys_to_virt(frame.start_address());
    unsafe { core::slice::from_raw_parts_mut(v.as_mut_ptr::<u8>(), 4096) }
}

fn build_inner(
    kmapper: &mut OffsetPageTable,
    track: &mut Track,
    name: &str,
    code: &[u8],
    args: &str,
) -> Result<Box<Proc>, &'static str> {
    let mut base = USER_BASE.load(Ordering::SeqCst);
    if base == 0 {
        let slot = (1..256usize)
            .find(|&i| kmapper.level_4_table()[i].is_unused())
            .ok_or("no free top-level page-table slot")?;
        base = (slot as u64) << 39;
        USER_BASE.store(base, Ordering::SeqCst);
    }

    // A fresh top-level table containing a copy of the kernel's entries.
    let pml4 = track.allocate_frame().ok_or("out of physical frames")?;
    let table: &'static mut PageTable =
        unsafe { &mut *crate::memory::phys_to_virt(pml4.start_address()).as_mut_ptr::<PageTable>() };
    table.zero();
    let kt = kmapper.level_4_table();
    for i in 0..512 {
        if !kt[i].is_unused() {
            table[i] = kt[i].clone();
        }
    }
    let mut mapper =
        unsafe { OffsetPageTable::new(table, VirtAddr::new(crate::memory::phys_offset())) };

    let leaf = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    // Code: user-readable, NOT writable.
    let mut code_frames = [PhysFrame::containing_address(PhysAddr::new(0)); CODE_PAGES];
    for i in 0..CODE_PAGES {
        let f = track.allocate_frame().ok_or("out of physical frames")?;
        map_one(&mut mapper, track, base + (i as u64) * 4096, f, leaf)?;
        code_frames[i] = f;
    }
    // Stack: writable but NEVER executable (W^X).
    let mut stack_frames = [PhysFrame::containing_address(PhysAddr::new(0)); STACK_PAGES];
    for i in 0..STACK_PAGES {
        let f = track.allocate_frame().ok_or("out of physical frames")?;
        map_one(
            &mut mapper,
            track,
            base + STACK_OFFSET + (i as u64) * 4096,
            f,
            leaf | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE,
        )?;
        stack_frames[i] = f;
    }

    for f in code_frames.iter().chain(stack_frames.iter()) {
        frame_bytes(*f).fill(0);
    }
    for (i, chunk) in code.chunks(4096).enumerate() {
        frame_bytes(code_frames[i])[..chunk.len()].copy_from_slice(chunk);
    }

    Ok(Box::new(Proc {
        tid: 0,
        name: String::from(name),
        code_len: code.len(),
        pml4,
        frames: Vec::new(), // filled in by the caller from `track.got`
        saved_rsp: 0,
        args: String::from(args),
        fault_kind: 0,
        fault_addr: 0,
        fault_rip: 0,
    }))
}

/// Build a complete address space + process record for `code`.
fn build_process(name: &str, code: &[u8], args: &str) -> Result<Box<Proc>, &'static str> {
    crate::memory::with_mem(|kmapper, frames| {
        let mut track = Track { inner: frames, got: Vec::new() };
        match build_inner(kmapper, &mut track, name, code, args) {
            Ok(mut p) => {
                p.frames = core::mem::take(&mut track.got);
                Ok(p)
            }
            Err(e) => {
                for f in track.got.drain(..) {
                    track.inner.release(f);
                }
                Err(e)
            }
        }
    })
    .ok_or("memory manager not installed")?
}

/// Hand every frame of a finished process back to the allocator.
fn free_process(p: Box<Proc>) {
    let frames = p.frames;
    crate::memory::with_mem(|_, fa| {
        for f in frames {
            fa.release(f);
        }
    });
}

/// Called by the scheduler (interrupts off) just before it switches to thread
/// `tid`: install that thread's address space and kernel-entry stack.
pub fn on_switch_to(tid: usize) {
    use x86_64::registers::control::Cr3;
    let (target, rsp0) = {
        let procs = PROCS.lock();
        match procs.iter().find(|p| p.tid == tid) {
            Some(p) => (p.pml4, p.saved_rsp),
            None => (crate::memory::kernel_cr3(), 0),
        }
    };
    let (cur, flags) = Cr3::read();
    if cur != target {
        unsafe { Cr3::write(target, flags) };
    }
    if rsp0 != 0 {
        // Interrupts/syscalls from ring 3 must land on THIS process's stack.
        unsafe {
            crate::gdt::rsp0_slot().write_unaligned(rsp0);
            core::ptr::write_volatile(core::ptr::addr_of_mut!(fast_kernel_rsp), rsp0);
        }
    }
}

/// Free the address spaces of processes whose thread was killed (`kill`) and
/// so never got to clean up after itself. A table that is the ACTIVE one right
/// now can't be freed (we'd be pulling the floor out) -- it's collected later.
pub fn collect() {
    use x86_64::registers::control::Cr3;
    let cur = Cr3::read().0;
    let dead: Vec<Box<Proc>> = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut g = PROCS.lock();
        let mut out = Vec::new();
        let mut i = 0;
        while i < g.len() {
            if !crate::scheduler::is_alive(g[i].tid) && g[i].pml4 != cur {
                out.push(g.remove(i));
            } else {
                i += 1;
            }
        }
        out
    });
    for p in dead {
        free_process(p);
    }
}

/// Is `[ptr, ptr+len)` entirely inside the running program's own mapped code
/// or stack pages? (The unmapped gap between them is NOT acceptable: a kernel
/// read there would page-fault in ring 0.)
fn user_range_ok(ptr: u64, len: u64) -> bool {
    let base = USER_BASE.load(Ordering::SeqCst);
    if base == 0 {
        return false;
    }
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    let in_code = ptr >= base && end <= base + CODE_BYTES;
    let sbase = base + STACK_OFFSET;
    let in_stack = ptr >= sbase && end <= sbase + STACK_BYTES;
    in_code || in_stack
}

// --- running a program -------------------------------------------------------

/// How a user program ended.
pub enum Exit {
    Code(u64),
    /// Killed by the CPU: (what happened, faulting address or error code, rip).
    Fault(&'static str, u64, u64),
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "unsupported" }
}

/// Run the calling thread's process in ring 3 until it exits or faults.
/// (The thread's process record and address space were built by `spawn_bytes`;
/// the scheduler already switched CR3 to it when it picked this thread.)
fn run_current() -> Result<Exit, &'static str> {
    let saved_ptr = with_current(|p| &mut p.saved_rsp as *mut u64).ok_or("no process record")?;
    let base = USER_BASE.load(Ordering::SeqCst);
    let (cs, ss) = crate::gdt::user_selectors();
    let user_rsp = base + STACK_OFFSET + STACK_BYTES;

    let status = unsafe {
        enter_user(base, user_rsp, saved_ptr, cs as u64, ss as u64, crate::gdt::rsp0_slot())
    };
    // We arrive back here from inside an interrupt gate, i.e. with IF clear.
    x86_64::instructions::interrupts::enable();

    if status == FAULT_STATUS {
        let (kind, addr, rip) =
            with_current(|p| (p.fault_kind, p.fault_addr, p.fault_rip)).unwrap_or((0, 0, 0));
        let what = match kind {
            1 => "page fault",
            2 => "general protection fault",
            3 => "invalid opcode",
            4 => "divide error",
            5 => "page fault: instruction fetch from a no-execute page (NX)",
            6 => "page fault: write to a read-only page",
            _ => "unknown fault",
        };
        Ok(Exit::Fault(what, addr, rip))
    } else {
        Ok(Exit::Code(status))
    }
}

/// Called from exception handlers that find the fault came from ring 3: record
/// what happened and abandon the program, returning to the kernel thread that
/// launched it. Never returns.
pub fn user_fault(kind: usize, addr: u64, rip: u64) -> ! {
    let saved = with_current(|p| {
        p.fault_kind = kind;
        p.fault_addr = addr;
        p.fault_rip = rip;
        p.saved_rsp
    })
    .unwrap_or(0);
    if saved == 0 {
        panic!("user fault with no process (kind {})", kind);
    }
    unsafe { kernel_resume(saved, FAULT_STATUS) }
}

// --- system calls ------------------------------------------------------------

#[no_mangle]
extern "C" fn syscall_dispatch(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    match nr {
        SYS_EXIT => {
            let saved = with_current(|p| p.saved_rsp).unwrap_or(0);
            unsafe { kernel_resume(saved, a1 & 0xFFFF_FFFF) }
        }
        SYS_WRITE => sys_write(a1, a2),
        SYS_TICKS => crate::interrupts::ticks(),
        SYS_GETPID => crate::scheduler::current_id() as u64,
        SYS_SLEEP => {
            crate::scheduler::sleep_ticks(a1.min(100));
            0
        }
        SYS_READFILE => sys_readfile(a1, a2, a3, a4),
        SYS_WRITEFILE => sys_writefile(a1, a2, a3, a4),
        SYS_GETARGS => sys_getargs(a1, a2),
        SYS_READKEY => sys_readkey(),
        SYS_SEND => sys_send(a1, a2, a3, a4),
        SYS_RECV => sys_recv(a1, a2, a3, a4),
        SYS_SPAWN => sys_spawn(a1, a2, a3, a4),
        _ => u64::MAX, // ENOSYS
    }
}

// --- touching user memory safely ----------------------------------------------
//
// With SMAP on, the CPU faults if the KERNEL reads or writes a user-accessible
// page -- unless RFLAGS.AC is set (`stac`; cleared again by `clac`). That turns
// every accidental kernel dereference of a user pointer into an immediate
// fault, and makes the deliberate ones (these two functions) easy to audit:
// they are the ONLY places the kernel touches user memory. Interrupts are off
// throughout a syscall, so AC can't leak into another context.

static SMAP_ON: AtomicBool = AtomicBool::new(false);

#[inline(always)]
unsafe fn stac() {
    if SMAP_ON.load(Ordering::Relaxed) {
        core::arch::asm!("stac", options(nomem, nostack));
    }
}

#[inline(always)]
unsafe fn clac() {
    if SMAP_ON.load(Ordering::Relaxed) {
        core::arch::asm!("clac", options(nomem, nostack));
    }
}

/// Copy `len` bytes out of user memory. Caller must have validated the range.
fn copy_from_user(ptr: u64, len: usize) -> Vec<u8> {
    let mut v = alloc::vec![0u8; len];
    unsafe {
        stac();
        core::ptr::copy_nonoverlapping(ptr as *const u8, v.as_mut_ptr(), len);
        clac();
    }
    v
}

/// Copy `data` into user memory. Caller must have validated the range.
fn copy_to_user(ptr: u64, data: &[u8]) {
    unsafe {
        stac();
        core::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
        clac();
    }
}

fn sys_write(ptr: u64, len: u64) -> u64 {
    if len > 2048 || !user_range_ok(ptr, len) {
        return u64::MAX;
    }
    let bytes = copy_from_user(ptr, len as usize);
    for &b in &bytes {
        // Only printable ASCII and newlines: never let a program inject
        // arbitrary bytes into the terminal.
        let c = if b == b'\n' || (0x20..0x7f).contains(&b) { b as char } else { '.' };
        print!("{}", c);
        serial_print!("{}", c);
    }
    len
}

/// Like `user_range_ok`, but for buffers the kernel will WRITE into: only the
/// stack is writable memory (the code pages are read-only to the program).
fn user_stack_ok(ptr: u64, len: u64) -> bool {
    let base = USER_BASE.load(Ordering::SeqCst);
    if base == 0 {
        return false;
    }
    let sbase = base + STACK_OFFSET;
    match ptr.checked_add(len) {
        Some(end) => ptr >= sbase && end <= sbase + STACK_BYTES,
        None => false,
    }
}

fn user_str(ptr: u64, len: u64) -> Option<String> {
    if len == 0 || len > 128 || !user_range_ok(ptr, len) {
        return None;
    }
    String::from_utf8(copy_from_user(ptr, len as usize)).ok()
}

/// readfile(name_ptr, name_len, buf_ptr, buf_len) -> bytes copied, or -1.
fn sys_readfile(np: u64, nl: u64, bp: u64, bl: u64) -> u64 {
    let name = match user_str(np, nl) {
        Some(n) => n,
        None => return u64::MAX,
    };
    if !user_stack_ok(bp, bl) {
        return u64::MAX;
    }
    match crate::fs::read(&name) {
        Some(text) => {
            let n = text.len().min(bl as usize);
            copy_to_user(bp, &text.as_bytes()[..n]);
            n as u64
        }
        None => u64::MAX,
    }
}

/// writefile(name_ptr, name_len, data_ptr, data_len) -> data_len, or -1.
/// Contents must be UTF-8 text (the filesystem stores Strings).
fn sys_writefile(np: u64, nl: u64, dp: u64, dl: u64) -> u64 {
    let name = match user_str(np, nl) {
        Some(n) => n,
        None => return u64::MAX,
    };
    if dl > 2048 || !user_range_ok(dp, dl) {
        return u64::MAX;
    }
    let data = copy_from_user(dp, dl as usize);
    match core::str::from_utf8(&data) {
        Ok(text) => {
            crate::fs::write(&name, text);
            dl
        }
        Err(_) => u64::MAX,
    }
}

/// getargs(buf_ptr, buf_len) -> bytes copied (0 if the program got no args).
fn sys_getargs(bp: u64, bl: u64) -> u64 {
    if !user_stack_ok(bp, bl) {
        return u64::MAX;
    }
    let args = with_current(|p| p.args.clone()).unwrap_or_default();
    let n = args.len().min(bl as usize);
    copy_to_user(bp, &args.as_bytes()[..n]);
    n as u64
}

// --- inter-process communication: named channels ----------------------------
//
// A "channel" is a byte pipe identified by a short name, created the first
// time anyone sends or receives on it and kept around after (so a consumer
// launched second can still find a channel a producer launched first already
// created). `send` blocks (sleeping, not spinning) while the channel is full;
// `recv` blocks while it is empty. Neither call holds the queue lock while
// blocked -- each retries the lock fresh every tick, the same pattern
// `readkey` uses for the keyboard.

const CHANNEL_CAPACITY: usize = 1024;
const MAX_CHANNELS: usize = 8;

static CHANNELS: Mutex<Vec<(String, alloc::collections::VecDeque<u8>)>> = Mutex::new(Vec::new());

fn with_channel<R>(name: &str, create: bool, f: impl FnOnce(&mut alloc::collections::VecDeque<u8>) -> R) -> Option<R> {
    let mut chans = CHANNELS.lock();
    if let Some(c) = chans.iter_mut().find(|(n, _)| n == name) {
        return Some(f(&mut c.1));
    }
    if !create || chans.len() >= MAX_CHANNELS {
        return None;
    }
    chans.push((String::from(name), alloc::collections::VecDeque::new()));
    let last = chans.last_mut().unwrap();
    Some(f(&mut last.1))
}

/// send(name_ptr, name_len, data_ptr, data_len) -> bytes sent, or -1.
/// Blocks (sleeping) while the channel is full; sends as much as fits each
/// time it wakes, so a send larger than the whole channel still completes.
fn sys_send(np: u64, nl: u64, dp: u64, dl: u64) -> u64 {
    let name = match user_str(np, nl) {
        Some(n) => n,
        None => return u64::MAX,
    };
    if dl > 65536 || !user_range_ok(dp, dl) {
        return u64::MAX;
    }
    let data = copy_from_user(dp, dl as usize);
    let mut sent = 0usize;
    while sent < data.len() {
        let took = x86_64::instructions::interrupts::without_interrupts(|| {
            with_channel(&name, true, |q| {
                let room = CHANNEL_CAPACITY.saturating_sub(q.len());
                let n = room.min(data.len() - sent);
                q.extend(&data[sent..sent + n]);
                n
            })
        });
        match took {
            Some(0) | None => crate::scheduler::sleep_ticks(1),
            Some(n) => sent += n,
        }
    }
    sent as u64
}

/// recv(name_ptr, name_len, buf_ptr, buf_len) -> bytes received (>=1), or -1.
/// Blocks (sleeping) until the channel exists and has at least one byte.
fn sys_recv(np: u64, nl: u64, bp: u64, bl: u64) -> u64 {
    let name = match user_str(np, nl) {
        Some(n) => n,
        None => return u64::MAX,
    };
    if !user_stack_ok(bp, bl) {
        return u64::MAX;
    }
    loop {
        let got = x86_64::instructions::interrupts::without_interrupts(|| {
            with_channel(&name, true, |q| {
                let n = q.len().min(bl as usize);
                if n == 0 {
                    return None;
                }
                let bytes: Vec<u8> = q.drain(..n).collect();
                Some(bytes)
            })
        })
        .flatten();
        if let Some(bytes) = got {
            copy_to_user(bp, &bytes);
            return bytes.len() as u64;
        }
        crate::scheduler::sleep_ticks(1);
    }
}

/// spawn(name_ptr, name_len, args_ptr, args_len) -> new pid, or -1.
///
/// A ring-3 program launching another ring-3 program: the name is looked up
/// exactly the way the shell's `run`/`exec` do (a built-in first, else an
/// `.axe` file with that exact name in the filesystem). The new process gets
/// its own address space and runs independently -- the caller is NOT its
/// parent in any special sense (no wait/reap here yet, see the limits below).
fn sys_spawn(np: u64, nl: u64, ap: u64, al: u64) -> u64 {
    let name = match user_str(np, nl) {
        Some(n) if !n.is_empty() => n,
        _ => return u64::MAX,
    };
    let args = if al == 0 {
        String::new()
    } else {
        match user_str(ap, al) {
            Some(a) => a,
            None => return u64::MAX,
        }
    };
    let code = match builtin(&name) {
        Some(c) => c.to_vec(),
        None => match crate::fs::read(&name).and_then(|t| parse_axe(&t).ok()) {
            Some(c) => c,
            None => return u64::MAX,
        },
    };
    match spawn_bytes(&name, code, &args) {
        Ok(id) => id as u64,
        Err(_) => u64::MAX,
    }
}

// --- keyboard input for user programs ---------------------------------------
//
// Keys normally go to the shell. While a user program is *blocked in readkey*
// the keyboard handler hands keys to it instead (`offer_key`), so the shell
// (and `kill`) keeps working whenever the program isn't asking for input.

static KEYQ: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static WAITING_FOR_KEY: AtomicBool = AtomicBool::new(false);

/// Called by the keyboard interrupt handler (interrupts off). Returns true if
/// a user program consumed the key.
pub fn offer_key(c: char) -> bool {
    if !WAITING_FOR_KEY.load(Ordering::SeqCst) {
        return false;
    }
    let mut q = KEYQ.lock();
    if q.len() < 64 {
        q.push(if c.is_ascii() { c as u8 } else { b'?' });
    }
    true
}

fn sys_readkey() -> u64 {
    loop {
        let got = {
            let mut q = KEYQ.lock();
            if q.is_empty() { None } else { Some(q.remove(0)) }
        };
        if let Some(k) = got {
            WAITING_FOR_KEY.store(false, Ordering::SeqCst);
            return k as u64;
        }
        WAITING_FOR_KEY.store(true, Ordering::SeqCst);
        crate::scheduler::sleep_ticks(1);
    }
}

// --- launching from the shell -------------------------------------------------

// --- the on-disk program format ("AXE1") -----------------------------------------
//
// A program in the filesystem is plain text, so it can be written with `write`,
// listed with `ls`, read with `cat`, and saved to the data disk like any file:
//
//     AXE1                       <- magic (rejects random text files)
//     # comments run to end of line
//     b8 00 00 00 00             <- x86-64 machine code, hex bytes,
//     bf 07 00 00 00                any whitespace / grouping
//     cd 80
//
// The loader validates before anything runs: magic, hex digits, non-empty,
// and the size limit.

/// Parse an AXE1 text file into machine code.
pub fn parse_axe(text: &str) -> Result<Vec<u8>, &'static str> {
    let mut tokens = text
        .lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .flat_map(|l| l.split_whitespace());
    if tokens.next() != Some("AXE1") {
        return Err("not an AXE1 program (missing magic \"AXE1\")");
    }
    let mut code = Vec::new();
    for tok in tokens {
        if tok.len() % 2 != 0 {
            return Err("odd number of hex digits in a token");
        }
        let digits = tok.as_bytes();
        for pair in digits.chunks(2) {
            let hi = (pair[0] as char).to_digit(16).ok_or("invalid hex digit")?;
            let lo = (pair[1] as char).to_digit(16).ok_or("invalid hex digit")?;
            code.push((hi * 16 + lo) as u8);
            if code.len() > CODE_BYTES as usize {
                return Err("program too large (max 16 KiB)");
            }
        }
    }
    if code.is_empty() {
        return Err("program has no code");
    }
    Ok(code)
}

/// Render machine code as an AXE1 text file (16 bytes per line).
pub fn to_axe(name: &str, desc: &str, code: &[u8]) -> String {
    use core::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "AXE1");
    let _ = writeln!(s, "# {} - {}", name, desc);
    for row in code.chunks(16) {
        for b in row {
            let _ = write!(s, "{:02x} ", b);
        }
        s.push('\n');
    }
    s
}

/// Kernel thread body: run this thread's process in ring 3, report how it
/// ended, then tear its address space down.
extern "C" fn user_thread(_: usize) {
    let pid = crate::scheduler::current_id();
    let (name, len) = with_current(|p| (p.name.clone(), p.code_len)).expect("process record");
    crate::println!("[user] pid {} '{}' -> ring 3 ({} bytes, private address space)", pid, name, len);
    crate::serial_println!("[user] pid {} '{}' -> ring 3 ({} bytes, private address space)", pid, name, len);

    let result = run_current();
    match result {
        Ok(Exit::Code(c)) => {
            crate::println!("[user] pid {} exited with status {}", pid, c);
            crate::serial_println!("[user] pid {} exited with status {}", pid, c);
        }
        Ok(Exit::Fault(what, addr, rip)) => {
            crate::println!(
                "[user] pid {} KILLED: {} (addr/code {:#x}, rip {:#x}) - kernel unharmed",
                pid, what, addr, rip
            );
            crate::serial_println!(
                "[user] pid {} KILLED: {} (addr/code {:#x}, rip {:#x}) - kernel unharmed",
                pid, what, addr, rip
            );
        }
        Err(e) => {
            crate::println!("[user] could not run: {}", e);
            crate::serial_println!("[user] could not run: {}", e);
        }
    }

    // Tear down: drop the process record, go back to the kernel's tables, and
    // only then free the frames (never free the page tables we're running on).
    let mine = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut g = PROCS.lock();
        let pos = g.iter().position(|p| p.tid == pid);
        let p = pos.map(|i| g.remove(i));
        let kcr3 = crate::memory::kernel_cr3();
        let (_, flags) = x86_64::registers::control::Cr3::read();
        unsafe { x86_64::registers::control::Cr3::write(kcr3, flags) };
        p
    });
    if let Some(p) = mine {
        free_process(p);
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        KEYQ.lock().clear();
    });
    WAITING_FOR_KEY.store(false, Ordering::SeqCst);
    crate::shell::prompt();
}

/// One-time CPU setup the first time a program is launched.
fn init_once() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::SeqCst) {
        let (nx, smep, smap) = harden();
        crate::println!("[harden] NX {}  SMEP {}  SMAP {}", on_off(nx), on_off(smep), on_off(smap));
        crate::serial_println!("[harden] NX {}  SMEP {}  SMAP {}", on_off(nx), on_off(smep), on_off(smap));
    }
    init_syscall_msrs();
}

/// Start `code` as a user process: build its private address space, then start
/// a kernel thread that will enter ring 3.
pub fn spawn_bytes(name: &str, code: Vec<u8>, args: &str) -> Result<usize, &'static str> {
    if code.len() > CODE_BYTES as usize {
        return Err("program too large (max 16 KiB)");
    }
    init_once();
    collect();
    if x86_64::instructions::interrupts::without_interrupts(|| PROCS.lock().len()) >= MAX_PROCS {
        return Err("too many user programs running (max 4)");
    }
    let proc_ = build_process(name, &code, args)?;
    // Register the process and start its thread as one atomic step, so the
    // thread can never run before its record exists.
    Ok(x86_64::instructions::interrupts::without_interrupts(|| {
        let id = crate::scheduler::spawn(name, user_thread, 0);
        let mut p = proc_;
        p.tid = id;
        PROCS.lock().push(p);
        id
    }))
}

/// Start the named built-in program.
pub fn spawn_builtin(name: &str, args: &str) -> Result<usize, &'static str> {
    let code = builtin(name).ok_or("no such program (try: run)")?;
    spawn_bytes(name, code.to_vec(), args)
}

/// Write every built-in program into the filesystem as `<name>.axe`.
pub fn install_all() -> usize {
    for (name, desc) in PROGRAMS {
        let code = builtin(name).unwrap();
        let mut file = String::from(*name);
        file.push_str(".axe");
        crate::fs::write(&file, &to_axe(name, desc, code));
    }
    PROGRAMS.len()
}
