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
//! A thread gets suspended in one of two ways, and `switch_context` doesn't
//! care which:
//!   * **preempted** — the timer interrupt handler calls `on_tick`, so the
//!     thread is parked mid-interrupt, and resumes by finishing the handler and
//!     `iretq`-ing to the instruction the timer interrupted;
//!   * **voluntarily** — the thread itself calls `sleep_ticks`, which calls
//!     `switch_context` directly and resumes by simply returning.
//!
//! ## Brand-new threads
//!
//! A thread that has never run has no register state to restore, so `spawn`
//! fabricates a stack that *looks like* a suspended thread: six zeroed
//! callee-saved slots and a return address pointing at `thread_start`, which
//! enables interrupts and calls the thread's entry function.
//!
//! ## Lifecycle
//!
//! `Ready`/`Running` threads take turns. `sleep_ticks` parks a thread as
//! `Sleeping` until a wake-up tick; `kill` marks a thread `Dead`; `reap` frees
//! the stacks of dead threads (never the one currently executing).

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
    Sleeping,
    Dead,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Ready => "ready",
            State::Sleeping => "sleeping",
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
    /// For `Sleeping` threads: the timer tick at which to wake up.
    wake_tick: u64,
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

/// A progress counter per thread id (modulo the table size), so demo threads
/// can show "I'm alive and doing work" without any shared-state plumbing.
const PROGRESS_SLOTS: usize = 32;
static PROGRESS: [AtomicU64; PROGRESS_SLOTS] = [const { AtomicU64::new(0) }; PROGRESS_SLOTS];

pub fn progress(id: usize) -> u64 {
    PROGRESS[id % PROGRESS_SLOTS].load(Ordering::Relaxed)
}

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
            wake_tick: 0,
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
        PROGRESS[id % PROGRESS_SLOTS].store(0, Ordering::Relaxed);
        s.threads.push(Box::new(Thread {
            id,
            name: String::from(name),
            state: State::Ready,
            saved_rsp: rsp,
            switches_in: 0,
            wake_tick: 0,
            _stack: Some(stack),
        }));
        id
    })
}

/// Pick the next ready thread (round-robin from the current one) and switch to
/// it. Interrupts must already be disabled. Wakes any sleepers whose time has
/// come first. If nothing else is ready, returns and the caller keeps running.
fn schedule() {
    let now = crate::interrupts::ticks();
    let (old_ptr, new_rsp) = {
        let mut s = SCHED.lock();
        for t in s.threads.iter_mut() {
            if t.state == State::Sleeping && t.wake_tick <= now {
                t.state = State::Ready;
            }
        }
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

/// Called from the timer interrupt handler (interrupts already disabled, EOI
/// already sent): preempt the running thread.
pub fn on_tick() {
    schedule();
}

/// Block the calling thread for `ticks` timer ticks (~55 ms each) and let
/// other threads run in the meantime. Unlike spinning, a sleeping thread uses
/// no CPU at all until its wake-up tick.
pub fn sleep_ticks(ticks: u64) {
    interrupts::without_interrupts(|| {
        {
            let mut s = SCHED.lock();
            let c = s.current;
            s.threads[c].state = State::Sleeping;
            s.threads[c].wake_tick = crate::interrupts::ticks() + ticks;
        }
        // We are no longer Running, so `schedule` will pick someone else and we
        // resume — right here — once a later tick has woken us and the
        // scheduler rotates back around to us.
        schedule();
    });
    // (`without_interrupts` restores the interrupt flag on the way out.)
}

/// Is there a live (not dead) thread with this id?
pub fn is_alive(id: usize) -> bool {
    interrupts::without_interrupts(|| {
        SCHED
            .lock()
            .threads
            .iter()
            .any(|t| t.id == id && t.state != State::Dead)
    })
}

/// Id of the thread that is executing right now.
pub fn current_id() -> usize {
    interrupts::without_interrupts(|| {
        let s = SCHED.lock();
        s.threads[s.current].id
    })
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

#[derive(Debug)]
pub enum KillError {
    NoSuchThread,
    IsBootThread,
    AlreadyDead,
}

/// Terminate a thread by id. The thread is simply never scheduled again; its
/// stack is reclaimed by `reap` (immediately, unless it is the thread that is
/// running right now — then at the next reap).
///
/// Caveat, stated plainly: killing a thread that holds a lock leaves that
/// lock held forever. Nothing here takes a lock across a preemption point, so
/// the demo threads are safe to kill — but this is not a general guarantee.
pub fn kill(id: usize) -> Result<(), KillError> {
    if id == 0 {
        return Err(KillError::IsBootThread);
    }
    let result = interrupts::without_interrupts(|| {
        let mut s = SCHED.lock();
        match s.threads.iter_mut().find(|t| t.id == id) {
            None => Err(KillError::NoSuchThread),
            Some(t) if t.state == State::Dead => Err(KillError::AlreadyDead),
            Some(t) => {
                t.state = State::Dead;
                Ok(())
            }
        }
    });
    reap();
    result
}

/// Free the stacks of dead threads. The thread currently running is never
/// freed, even if dead — we may be executing on its stack right now.
pub fn reap() {
    interrupts::without_interrupts(|| {
        let mut s = SCHED.lock();
        let cur_id = s.threads[s.current].id;
        s.threads.retain(|t| t.state != State::Dead || t.id == cur_id);
        s.current = s.threads.iter().position(|t| t.id == cur_id).unwrap_or(0);
    });
}

pub struct ThreadInfo {
    pub id: usize,
    pub name: String,
    pub state: State,
    pub switches: u64,
    pub wake_in: u64,
}

pub fn snapshot() -> Vec<ThreadInfo> {
    reap();
    let now = crate::interrupts::ticks();
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
                wake_in: if t.state == State::Sleeping {
                    t.wake_tick.saturating_sub(now)
                } else {
                    0
                },
            })
            .collect()
    })
}

// --- demo thread bodies ---------------------------------------------------------

fn bump_progress() {
    PROGRESS[current_id() % PROGRESS_SLOTS].fetch_add(1, Ordering::Relaxed);
}

/// A CPU hog: counts in a tight loop and never yields.
pub extern "C" fn spin_entry(_: usize) {
    let slot = current_id() % PROGRESS_SLOTS;
    loop {
        PROGRESS[slot].fetch_add(1, Ordering::Relaxed);
    }
}

/// A well-behaved background thread: sleeps ~1 second (18 ticks), wakes, logs,
/// repeats. While it sleeps it costs nothing.
pub extern "C" fn ticker_entry(_: usize) {
    let id = current_id();
    loop {
        sleep_ticks(18);
        bump_progress();
        let n = progress(id);
        crate::println!("[ticker {}] woke up, beat #{} (tick {})", id, n, crate::interrupts::ticks());
        crate::serial_println!("[ticker {}] woke up, beat #{} (tick {})", id, n, crate::interrupts::ticks());
    }
}
