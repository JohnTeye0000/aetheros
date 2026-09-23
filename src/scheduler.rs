//! Preemptive multitasking: kernel threads switched by the timer interrupt.
//!
//! Stage 6's executor was *cooperative*: a task ran until it chose to
//! `yield_now().await`. A task that never yielded would hog the CPU forever.
//! Here the kernel takes the CPU away by force: every timer tick, whatever
//! thread is running gets suspended and the next ready thread resumes, whether
//! or not either one ever asked for it.
//!
//! ## How a context switch works
//!
//! A thread's entire "state" while it isn't running is: its own stack, plus
//! one saved stack pointer. `switch_context` (assembly, below) pushes the
//! callee-saved registers onto the *old* thread's stack, stores the old stack
//! pointer, loads the new thread's saved stack pointer, pops *its* registers
//! and `ret`s — which returns into wherever that thread last called
//! `switch_context` from. From each thread's point of view, `switch_context`
//! is just a function that takes a very long time to return.
//!
//! The switch happens **inside the timer interrupt handler**. So a suspended
//! thread is always parked mid-interrupt, with its interrupt frame still on
//! its own stack; when it's resumed it finishes the handler and `iretq`s back
//! to exactly the instruction the timer interrupted. That instruction never
//! knew it had been paused.
//!
//! ## Brand-new threads
//!
//! A thread that has never run has no register state to restore, so `spawn`
//! fabricates a stack that *looks like* a suspended thread: six zeroed
//! callee-saved slots and a return address pointing at `thread_start`, which
//! enables interrupts and calls the thread's entry function.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts;

/// Each thread gets its own stack. The shell runs *on whichever thread's stack
/// was interrupted*, so this has to be big enough for `format!` and friends.
const STACK_SIZE: usize = 32 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Running,
    Ready,
    Dead,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Ready => "ready",
            State::Dead => "dead",
        }
    }
}

struct Thread {
    id: usize,
    name: String,
    state: State,
    /// Where this thread's stack pointer was when it was last switched out.
    saved_rsp: usize,
    /// How many times the scheduler has switched *into* this thread.
    switches_in: u64,
    /// The thread's stack. `None` for the boot thread, which keeps using the
    /// stack the bootloader gave it.
    _stack: Option<Box<[u8]>>,
}

struct Scheduler {
    /// Boxed so each `Thread` has a stable address even when the Vec grows —
    /// `switch_context` writes into `saved_rsp` through a raw pointer.
    threads: Vec<Box<Thread>>,
    current: usize,
    next_id: usize,
}

/// Only ever locked with interrupts disabled (from inside interrupt handlers,
/// or wrapped in `without_interrupts`), so the timer can never fire while
/// someone on this single CPU holds it.
static SCHED: Mutex<Scheduler> = Mutex::new(Scheduler {
    threads: Vec::new(),
    current: 0,
    next_id: 1,
});

global_asm!(
    r#"
    .global switch_context
    switch_context:
        push rbp
        push rbx
        push r12
        push r13
        push r14
        push r15
        mov  [rdi], rsp
        mov  rsp, rsi
        pop  r15
        pop  r14
        pop  r13
        pop  r12
        pop  rbx
        pop  rbp
        ret

    .global thread_start
    thread_start:
        sti
        mov  rdi, r12
        call rbx
        call thread_exit_trampoline
    "#
);

extern "C" {
    fn switch_context(old_rsp: *mut usize, new_rsp: usize);
    fn thread_start();
}

#[no_mangle]
extern "C" fn thread_exit_trampoline() -> ! {
    exit_current()
}

/// Adopt the currently-running code (`kernel_main`) as thread 0.
pub fn init() {
    interrupts::without_interrupts(|| {
        let mut s = SCHED.lock();
        s.threads.push(Box::new(Thread {
            id: 0,
            name: String::from("kernel_main"),
            state: State::Running,
            saved_rsp: 0,
            switches_in: 0,
            _stack: None,
        }));
        s.current = 0;
    });
}

/// Create a new thread that will start running `entry(arg)` the next time the
/// scheduler picks it. Returns its id.
pub fn spawn(name: &str, entry: extern "C" fn(usize), arg: usize) -> usize {
    let stack: Box<[u8]> = vec![0u8; STACK_SIZE].into_boxed_slice();
    let top = (stack.as_ptr() as usize + STACK_SIZE) & !0xF;

    // Fabricate the frame `switch_context` will "restore": r15 r14 r13 r12
    // rbx rbp, then a return address. `thread_start` moves r12 into rdi (the
    // argument) and calls rbx (the entry). The layout puts `top` (16-aligned)
    // exactly where rsp lands after the `ret`, so the `call` in thread_start
    // sees a correctly aligned stack.
    let rsp = top - 56;
    unsafe {
        let p = rsp as *mut usize;
        p.add(0).write(0); // r15
        p.add(1).write(0); // r14
        p.add(2).write(0); // r13
        p.add(3).write(arg); // r12
        p.add(4).write(entry as *const () as usize); // rbx
        p.add(5).write(0); // rbp
        p.add(6).write(thread_start as *const () as usize); // return address
    }

    interrupts::without_interrupts(|| {
        let mut s = SCHED.lock();
        let id = s.next_id;
        s.next_id += 1;
        s.threads.push(Box::new(Thread {
            id,
            name: String::from(name),
            state: State::Ready,
            saved_rsp: rsp,
            switches_in: 0,
            _stack: Some(stack),
        }));
        id
    })
}

/// Called from the timer interrupt handler (interrupts already disabled, EOI
/// already sent): pick the next ready thread, round-robin, and switch to it.
pub fn on_tick() {
    let (old_ptr, new_rsp) = {
        let mut s = SCHED.lock();
        let n = s.threads.len();
        if n < 2 {
            return;
        }
        let cur = s.current;
        let next = match (1..=n)
            .map(|i| (cur + i) % n)
            .find(|&i| s.threads[i].state == State::Ready)
        {
            Some(i) => i,
            None => return, // nothing else to run
        };
        if s.threads[cur].state == State::Running {
            s.threads[cur].state = State::Ready;
        }
        s.threads[next].state = State::Running;
        s.threads[next].switches_in += 1;
        s.current = next;
        let old_ptr = &mut s.threads[cur].saved_rsp as *mut usize;
        (old_ptr, s.threads[next].saved_rsp)
    }; // lock released BEFORE switching: the thread we switch to will want it
    unsafe { switch_context(old_ptr, new_rsp) };
}

/// Mark the current thread finished. The scheduler never resumes it again.
pub fn exit_current() -> ! {
    interrupts::without_interrupts(|| {
        let mut s = SCHED.lock();
        let c = s.current;
        s.threads[c].state = State::Dead;
    });
    loop {
        x86_64::instructions::hlt();
    }
}

pub struct ThreadInfo {
    pub id: usize,
    pub name: String,
    pub state: State,
    pub switches: u64,
}

pub fn snapshot() -> Vec<ThreadInfo> {
    interrupts::without_interrupts(|| {
        SCHED
            .lock()
            .threads
            .iter()
            .map(|t| ThreadInfo {
                id: t.id,
                name: t.name.clone(),
                state: t.state,
                switches: t.switches_in,
            })
            .collect()
    })
}

// --- demo workers -------------------------------------------------------------

/// One counter per demo worker. Each worker does nothing but increment its own
/// counter in a tight loop — it never yields, never sleeps, never calls
/// anything. If all three counters climb, the only explanation is preemption.
pub static WORK: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

extern "C" fn worker(idx: usize) {
    loop {
        WORK[idx].fetch_add(1, Ordering::Relaxed);
    }
}

pub fn spawn_demo_workers() {
    spawn("worker-a", worker, 0);
    spawn("worker-b", worker, 1);
    spawn("worker-c", worker, 2);
}
