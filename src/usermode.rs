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
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::structures::paging::{Mapper, Page, PageTableFlags, PhysFrame, Size4KiB, FrameAllocator};
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

global_asm!(
    r#"
    // enter_user(entry rdi, user_rsp rsi, saved_slot rdx, cs rcx, ss r8, rsp0_slot r9) -> status
    .global enter_user
    enter_user:
        push rbp
        push rbx
        push r12
        push r13
        push r14
        push r15
        mov  [rdx], rsp          // where `exit` will come back to
        mov  [r9], rsp           // ring 3 -> ring 0 interrupts land just below this
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
}

/// Address of the `int 0x80` entry stub, for the IDT.
pub fn syscall_entry_addr() -> u64 {
    syscall_entry as *const () as usize as u64
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
];

pub fn builtin(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "hello" => blob!(user_hello_start, user_hello_end),
        "count" => blob!(user_count_start, user_count_end),
        "spin" => blob!(user_spin_start, user_spin_end),
        "hlt" => blob!(user_hlt_start, user_hlt_end),
        "poke" => blob!(user_poke_start, user_poke_end),
        "badptr" => blob!(user_badptr_start, user_badptr_end),
        _ => return None,
    })
}

// --- user address space ------------------------------------------------------

struct UserSpace {
    base: u64,
    code: [PhysFrame; CODE_PAGES],
    stack: [PhysFrame; STACK_PAGES],
}

static SPACE: Mutex<Option<UserSpace>> = Mutex::new(None);

/// Map the user code + stack pages the first time a program runs. They stay
/// mapped afterwards and are simply re-zeroed and re-filled for each program.
fn ensure_space() -> Result<(u64, [PhysFrame; CODE_PAGES], [PhysFrame; STACK_PAGES]), &'static str>
{
    if let Some(s) = SPACE.lock().as_ref() {
        return Ok((s.base, s.code, s.stack));
    }

    let built = crate::memory::with_mem(|mapper, frames| -> Result<UserSpace, &'static str> {
        // Find an entirely unused top-level slot in the lower half, so every
        // table we create for it is ours (and can safely be marked user-visible).
        let slot = (1..256usize)
            .find(|&i| mapper.level_4_table()[i].is_unused())
            .ok_or("no free top-level page-table slot")?;
        let base = (slot as u64) << 39;

        let leaf = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        let table = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

        let mut map_page = |addr: u64, flags: PageTableFlags| -> Result<PhysFrame, &'static str> {
            let frame = frames.allocate_frame().ok_or("out of physical frames")?;
            let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(addr));
            unsafe {
                mapper
                    .map_to_with_table_flags(page, frame, flags, table, frames)
                    .map_err(|_| "mapping a user page failed")?
                    .flush();
            }
            Ok(frame)
        };

        // Code: user-readable, NOT writable (a program can't rewrite itself).
        let mut code = [PhysFrame::containing_address(PhysAddr::new(0)); CODE_PAGES];
        for (i, slot) in code.iter_mut().enumerate() {
            *slot = map_page(base + (i as u64) * 4096, leaf)?;
        }
        // Stack: user-readable and writable.
        let mut stack = [PhysFrame::containing_address(PhysAddr::new(0)); STACK_PAGES];
        for (i, slot) in stack.iter_mut().enumerate() {
            *slot = map_page(base + STACK_OFFSET + (i as u64) * 4096, leaf | PageTableFlags::WRITABLE)?;
        }
        Ok(UserSpace { base, code, stack })
    });

    let space = built.ok_or("memory manager not installed")??;
    let out = (space.base, space.code, space.stack);
    *SPACE.lock() = Some(space);
    Ok(out)
}

/// Zero a physical frame through the kernel's physical-memory mapping.
fn frame_bytes(frame: PhysFrame) -> &'static mut [u8] {
    let v = crate::memory::phys_to_virt(frame.start_address());
    unsafe { core::slice::from_raw_parts_mut(v.as_mut_ptr::<u8>(), 4096) }
}

/// Is `[ptr, ptr+len)` entirely inside the running program's own mapped code
/// or stack pages? (The unmapped gap between them is NOT acceptable: a kernel
/// read there would page-fault in ring 0.)
fn user_range_ok(ptr: u64, len: u64) -> bool {
    let base = match SPACE.lock().as_ref() {
        Some(s) => s.base,
        None => return false,
    };
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

/// Where `enter_user` stashed the kernel stack pointer for `exit`/fault to
/// restore.
static SAVED_RSP: AtomicU64 = AtomicU64::new(0);

static FAULT_KIND: AtomicUsize = AtomicUsize::new(0);
static FAULT_ADDR: AtomicU64 = AtomicU64::new(0);
static FAULT_RIP: AtomicU64 = AtomicU64::new(0);

/// How a user program ended.
pub enum Exit {
    Code(u64),
    /// Killed by the CPU: (what happened, faulting address or error code, rip).
    Fault(&'static str, u64, u64),
}

/// Copy `code` into the user address space and run it to completion in ring 3.
/// Must be called from a kernel thread (it borrows that thread's stack for
/// ring 3 -> ring 0 transitions).
pub fn run(code: &[u8]) -> Result<Exit, &'static str> {
    if code.len() > CODE_BYTES as usize {
        return Err("program too large (max 16 KiB)");
    }
    let (base, code_frames, stack_frames) = ensure_space()?;

    for f in code_frames.iter().chain(stack_frames.iter()) {
        frame_bytes(*f).fill(0);
    }
    for (i, chunk) in code.chunks(4096).enumerate() {
        frame_bytes(code_frames[i])[..chunk.len()].copy_from_slice(chunk);
    }

    let (cs, ss) = crate::gdt::user_selectors();
    let user_rsp = base + STACK_OFFSET + STACK_BYTES;
    FAULT_KIND.store(0, Ordering::SeqCst);

    let status = unsafe {
        enter_user(
            base,
            user_rsp,
            SAVED_RSP.as_ptr(),
            cs as u64,
            ss as u64,
            crate::gdt::rsp0_slot(),
        )
    };
    // We arrive back here from inside an interrupt gate, i.e. with IF clear.
    x86_64::instructions::interrupts::enable();

    if status == FAULT_STATUS {
        let what = match FAULT_KIND.load(Ordering::SeqCst) {
            1 => "page fault",
            2 => "general protection fault",
            3 => "invalid opcode",
            4 => "divide error",
            _ => "unknown fault",
        };
        Ok(Exit::Fault(what, FAULT_ADDR.load(Ordering::SeqCst), FAULT_RIP.load(Ordering::SeqCst)))
    } else {
        Ok(Exit::Code(status))
    }
}

/// Called from exception handlers that find the fault came from ring 3: record
/// what happened and abandon the program, returning to the kernel thread that
/// launched it. Never returns.
pub fn user_fault(kind: usize, addr: u64, rip: u64) -> ! {
    FAULT_KIND.store(kind, Ordering::SeqCst);
    FAULT_ADDR.store(addr, Ordering::SeqCst);
    FAULT_RIP.store(rip, Ordering::SeqCst);
    unsafe { kernel_resume(SAVED_RSP.load(Ordering::SeqCst), FAULT_STATUS) }
}

// --- system calls ------------------------------------------------------------

#[no_mangle]
extern "C" fn syscall_dispatch(nr: u64, a1: u64, a2: u64, _a3: u64) -> u64 {
    match nr {
        SYS_EXIT => unsafe { kernel_resume(SAVED_RSP.load(Ordering::SeqCst), a1 & 0xFFFF_FFFF) },
        SYS_WRITE => sys_write(a1, a2),
        SYS_TICKS => crate::interrupts::ticks(),
        SYS_GETPID => crate::scheduler::current_id() as u64,
        SYS_SLEEP => {
            crate::scheduler::sleep_ticks(a1.min(100));
            0
        }
        _ => u64::MAX, // ENOSYS
    }
}

fn sys_write(ptr: u64, len: u64) -> u64 {
    if len > 512 || !user_range_ok(ptr, len) {
        return u64::MAX;
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    for &b in bytes {
        // Only printable ASCII and newlines: never let a program inject
        // arbitrary bytes into the terminal.
        let c = if b == b'\n' || (0x20..0x7f).contains(&b) { b as char } else { '.' };
        print!("{}", c);
        serial_print!("{}", c);
    }
    len
}

// --- launching from the shell -------------------------------------------------

static RUNNING: AtomicBool = AtomicBool::new(false);
static RUNNING_THREAD: AtomicUsize = AtomicUsize::new(0);

/// The program waiting for its thread to pick it up: `(name, machine code)`.
static PENDING: Mutex<Option<(String, Vec<u8>)>> = Mutex::new(None);

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

/// Kernel thread body: run the pending program in ring 3.
extern "C" fn user_thread(_: usize) {
    let (name, code) = x86_64::instructions::interrupts::without_interrupts(|| PENDING.lock().take())
        .expect("user thread started with no program");
    let pid = crate::scheduler::current_id();
    crate::println!("[user] pid {} '{}' -> ring 3 ({} bytes)", pid, name, code.len());
    crate::serial_println!("[user] pid {} '{}' -> ring 3 ({} bytes)", pid, name, code.len());

    let result = run(&code);
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
    RUNNING.store(false, Ordering::SeqCst);
    crate::shell::prompt();
}

/// Start `code` as a user process on its own kernel thread.
pub fn spawn_bytes(name: &str, code: Vec<u8>) -> Result<usize, &'static str> {
    if code.len() > CODE_BYTES as usize {
        return Err("program too large (max 16 KiB)");
    }
    if RUNNING.load(Ordering::SeqCst)
        && crate::scheduler::is_alive(RUNNING_THREAD.load(Ordering::SeqCst))
    {
        return Err("a user program is already running");
    }
    RUNNING.store(true, Ordering::SeqCst);
    x86_64::instructions::interrupts::without_interrupts(|| {
        *PENDING.lock() = Some((String::from(name), code));
    });
    let id = crate::scheduler::spawn(name, user_thread, 0);
    RUNNING_THREAD.store(id, Ordering::SeqCst);
    Ok(id)
}

/// Start the named built-in program.
pub fn spawn_builtin(name: &str) -> Result<usize, &'static str> {
    let code = builtin(name).ok_or("no such program (try: run)")?;
    spawn_bytes(name, code.to_vec())
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
