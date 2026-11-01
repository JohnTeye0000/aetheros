//! Preemptive multitasking: kernel threads switched by the timer interrupt.
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
//!   * **voluntarily** — the thread calls `sleep_ticks` or `block_current`,
//!     which call `switch_context` directly; it resumes by simply returning.
//!
//! ## Brand-new threads
//!
//! A thread that has never run has no register state to restore, so `spawn`
//! fabricates a stack that *looks like* a suspended thread: six zeroed
//! callee-saved slots and a return address pointing at `thread_start`, which
//! enables interrupts and calls the thread's entry function.
//!
//! ## Policies (Stage 35)
//!
//! Which ready thread runs next is decided by a pluggable **policy**, so the
//! same kernel can be measured under different scheduling rules:
//!
//! | policy  | priority comes from            | quantum             |
//! |---------|--------------------------------|---------------------|
//! | `rr`    | nobody: plain round robin      | Q for everyone      |
//! | `hint`  | the program's declared hint    | Q << level          |
//! | `mlfq`  | observed behaviour (MLFQ)      | Q << level          |
//! | `hmlfq` | hint as MLFQ's starting level  | Q << level          |
//! | `fair`  | nobody: CFS-style vruntime     | Q / runnable threads |
//! | `hfair` | as fair; hint only sizes slices | cpu-hinted: x HFAIR_MULT |
//!
//! `hint` uses only *static* information (what the program says it is);
//! `mlfq` uses only *runtime* information (what the program has been doing —
//! threads that burn whole quanta sink, threads that block stay high);
//! `hmlfq` combines them. Both `hint` and the MLFQs include an anti-starvation
//! rule (aging / periodic boost), and both are stated in the constants below.
//!
//! Thread 0 (`kernel_main`, sitting in a `hlt` loop) is the **idle thread**: it
//! only runs when nothing else is ready, so it never steals a time slice.
//!
//! ## Accounting
//!
//! Every thread records, in TSC cycles: when it was created, when it first
//! ran, total CPU time, total time spent ready-but-not-running, and a sample
//! of **wake-up latency** (time from being woken to actually running) for every
//! wake-up. These feed the benchmark harness in `bench.rs`.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts;

use crate::timing;

/// Each thread gets its own stack. The shell runs *on whichever thread's stack
/// was interrupted*, so this has to be big enough for `format!` and friends.
const STACK_SIZE: usize = 32 * 1024;

/// Wake-up latency samples kept per thread (oldest kept, extras dropped).
const MAX_LAT_SAMPLES: usize = 4096;

/// Number of priority levels for MLFQ (`levels=` in `bench`). The hint
/// policies always use 3 (interactive, io, cpu).
pub const DEFAULT_MLFQ_LEVELS: u8 = 3;
static MLFQ_LEVELS: AtomicU8 = AtomicU8::new(DEFAULT_MLFQ_LEVELS);
pub fn set_mlfq_levels(n: u8) {
    MLFQ_LEVELS.store(n.clamp(1, 8), Ordering::SeqCst);
}
fn mlfq_levels() -> u8 {
    MLFQ_LEVELS.load(Ordering::Relaxed)
}

/// MLFQ: every BOOST_MS, all threads return to the top level (so CPU-bound
/// threads can't starve forever, and a thread whose behaviour changed gets
/// re-evaluated).
pub const DEFAULT_BOOST_MS: u64 = 200;
static BOOST_MS: AtomicU64 = AtomicU64::new(DEFAULT_BOOST_MS);

/// `hint`: a thread that has been ready-but-not-running for AGING_MS is treated
/// as top priority until it runs (so a CPU-bound thread can't starve forever).
pub const DEFAULT_AGING_MS: u64 = 100;
static AGING_MS: AtomicU64 = AtomicU64::new(DEFAULT_AGING_MS);

/// Runtime knobs, so the study can sweep them (and so a benchmark line can state
/// them explicitly instead of inheriting whatever the last run left behind).
pub fn set_aging_ms(ms: u64) {
    AGING_MS.store(ms.max(1), Ordering::SeqCst);
}
pub fn set_boost_ms(ms: u64) {
    BOOST_MS.store(ms.max(1), Ordering::SeqCst);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Running,
    Ready,
    Sleeping,
    Blocked,
    Dead,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Ready => "ready",
            State::Sleeping => "sleeping",
            State::Blocked => "blocked",
            State::Dead => "dead",
        }
    }
}

/// What a program says about itself (Stage 35: carried in the program header).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hint {
    None,
    Cpu,
    Io,
    Interactive,
}

impl Hint {
    pub fn name(self) -> &'static str {
        match self {
            Hint::None => "none",
            Hint::Cpu => "cpu",
            Hint::Io => "io",
            Hint::Interactive => "interactive",
        }
    }
    pub fn parse(s: &str) -> Option<Hint> {
        Some(match s {
            "none" => Hint::None,
            "cpu" => Hint::Cpu,
            "io" => Hint::Io,
            "interactive" => Hint::Interactive,
            _ => return None,
        })
    }
    /// Priority level a hint maps to (0 = highest). `iotop=1` in `bench` puts
    /// io-hinted threads on the interactive level.
    fn level(self) -> u8 {
        match self {
            Hint::Interactive => 0,
            Hint::Io if HINT_IO_TOP.load(Ordering::Relaxed) => 0,
            Hint::Io | Hint::None => 1,
            Hint::Cpu => 2,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Policy {
    RoundRobin,
    Hinted,
    Mlfq,
    HintedMlfq,
    Fair,
    /// `fair` plus hints that only SIZE time slices (never set priority).
    HintedFair,
}

impl Policy {
    pub fn name(self) -> &'static str {
        match self {
            Policy::RoundRobin => "rr",
            Policy::Hinted => "hint",
            Policy::Mlfq => "mlfq",
            Policy::HintedMlfq => "hmlfq",
            Policy::Fair => "fair",
            Policy::HintedFair => "hfair",
        }
    }
    pub fn parse(s: &str) -> Option<Policy> {
        Some(match s {
            "rr" => Policy::RoundRobin,
            "hint" => Policy::Hinted,
            "mlfq" => Policy::Mlfq,
            "hmlfq" => Policy::HintedMlfq,
            "fair" => Policy::Fair,
            "hfair" => Policy::HintedFair,
            _ => return None,
        })
    }
    fn from_u8(v: u8) -> Policy {
        match v {
            1 => Policy::Hinted,
            2 => Policy::Mlfq,
            3 => Policy::HintedMlfq,
            4 => Policy::Fair,
            5 => Policy::HintedFair,
            _ => Policy::RoundRobin,
        }
    }
    /// Both virtual-runtime policies.
    pub fn is_fair(self) -> bool {
        matches!(self, Policy::Fair | Policy::HintedFair)
    }
    fn to_u8(self) -> u8 {
        match self {
            Policy::RoundRobin => 0,
            Policy::Hinted => 1,
            Policy::Mlfq => 2,
            Policy::HintedMlfq => 3,
            Policy::Fair => 4,
            Policy::HintedFair => 5,
        }
    }
}

static POLICY: AtomicU8 = AtomicU8::new(0);
/// Base quantum in timer ticks (1 tick = 1 ms).
static QUANTUM: AtomicU64 = AtomicU64::new(10);

static CTX_SWITCHES: AtomicU64 = AtomicU64::new(0);
static SCHED_CYCLES: AtomicU64 = AtomicU64::new(0);
static LAST_BOOST: AtomicU64 = AtomicU64::new(0);

pub fn policy() -> Policy {
    Policy::from_u8(POLICY.load(Ordering::Relaxed))
}
pub fn set_policy(p: Policy) {
    POLICY.store(p.to_u8(), Ordering::SeqCst);
}
pub fn quantum_ms() -> u64 {
    QUANTUM.load(Ordering::Relaxed)
}
pub fn set_quantum_ms(q: u64) {
    QUANTUM.store(q.clamp(1, 1000), Ordering::SeqCst);
}
/// Context switches performed since the last `reset_counters`.
pub fn ctx_switches() -> u64 {
    CTX_SWITCHES.load(Ordering::Relaxed)
}
/// TSC cycles spent inside the scheduler's decision code since the last reset.
pub fn sched_cycles() -> u64 {
    SCHED_CYCLES.load(Ordering::Relaxed)
}
/// Called at the start of every benchmark run. Also restarts MLFQ's boost
/// clock: without that, the boost phase at the start of a run depended on when
/// the PREVIOUS run ended, so identical runs (same seed) disagreed by up to
/// 30 ms in io-style p95 depending on their order in the plan (devlog 045).
pub fn reset_counters() {
    CTX_SWITCHES.store(0, Ordering::SeqCst);
    SCHED_CYCLES.store(0, Ordering::SeqCst);
    LAST_BOOST.store(crate::interrupts::ticks(), Ordering::SeqCst);
}

/// The 512-byte area `fxsave`/`fxrstor` use for the x87/MMX/SSE registers.
/// The CPU requires 16-byte alignment.
#[repr(C, align(16))]
struct FxArea([u8; 512]);

struct Thread {
    /// This thread's floating-point/SSE registers while it isn't running.
    fx: Box<FxArea>,
    id: usize,
    name: String,
    state: State,
    /// Where this thread's stack pointer was when it was last switched out.
    saved_rsp: usize,
    /// How many times the scheduler has switched *into* this thread.
    switches_in: u64,
    /// For `Sleeping` threads: the timer tick at which to wake up.
    wake_tick: u64,
    /// For `Blocked` threads: what they're waiting for (see `wake_all`).
    block_key: u64,
    /// The thread's stack. `None` for the boot thread, which keeps using the
    /// stack the bootloader gave it.
    _stack: Option<Box<[u8]>>,

    // --- policy state ---
    hint: Hint,
    /// The benchmark harness's own threads (launcher, autorun): always run
    /// first when ready, under every policy, so no workload can delay when
    /// another workload process is launched (see devlog 035).
    system: bool,
    level: u8,
    /// Ticks used since this thread was last dispatched.
    slice_used: u64,
    /// MLFQ: ticks used at the current level (survives blocking, so a thread
    /// can't stay high just by yielding right before its quantum ends).
    used_at_level: u64,

    // --- accounting, in TSC cycles ---
    created: u64,
    first_run: u64,
    dispatched_at: u64,
    cpu: u64,
    ready_since: u64,
    waited: u64,
    /// This Ready period started with a wake-up (sleep/block ended), so its
    /// length is a wake-up latency sample.
    woke: bool,
    preempts: u64,
    blocks: u64,
    lat: Vec<u64>,
    /// `fair`: CPU time used, adjusted on wake-up (TSC cycles). The fair
    /// policy always runs the runnable thread with the smallest.
    vruntime: u64,
}

impl Thread {
    fn new(id: usize, name: &str, state: State, saved_rsp: usize, stack: Option<Box<[u8]>>) -> Thread {
        let now = timing::now();
        Thread {
            fx: Box::new(FxArea([0; 512])),
            id,
            name: String::from(name),
            state,
            saved_rsp,
            switches_in: 0,
            wake_tick: 0,
            block_key: 0,
            _stack: stack,
            hint: Hint::None,
            system: false,
            level: 0,
            slice_used: 0,
            used_at_level: 0,
            created: now,
            first_run: 0,
            dispatched_at: 0,
            cpu: 0,
            ready_since: now,
            waited: 0,
            woke: false,
            preempts: 0,
            blocks: 0,
            lat: Vec::new(),
            vruntime: 0,
        }
    }

    fn make_ready(&mut self, now: u64, woke: bool) {
        self.state = State::Ready;
        self.ready_since = now;
        self.woke = woke;
    }
}

struct Scheduler {
    /// Boxed so each `Thread` has a stable address even when the Vec grows —
    /// `switch_context` writes into `saved_rsp` through a raw pointer.
    threads: Vec<Box<Thread>>,
    current: usize,
    next_id: usize,
    /// `fair`: a floor that only moves forward -- roughly the smallest
    /// vruntime among runnable threads. New and waking threads are placed
    /// relative to it.
    min_vruntime: u64,
}

/// Only ever locked with interrupts disabled (from inside interrupt handlers,
/// or wrapped in `without_interrupts`), so the timer can never fire while
/// someone on this single CPU holds it.
static SCHED: Mutex<Scheduler> = Mutex::new(Scheduler {
    threads: Vec::new(),
    current: 0,
    next_id: 1,
    min_vruntime: 0,
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

/// Let user programs use SSE (Stage 36). CR0.EM=0 (no x87 emulation trap),
/// CR0.MP=1, CR4.OSFXSR=1 (the OS saves SSE state with fxsave/fxrstor),
/// CR4.OSXMMEXCPT=1 (SIMD float exceptions are reported, not #UD).
fn enable_sse() {
    use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};
    unsafe {
        Cr0::update(|f| {
            f.remove(Cr0Flags::EMULATE_COPROCESSOR);
            f.insert(Cr0Flags::MONITOR_COPROCESSOR);
        });
        Cr4::update(|f| {
            f.insert(Cr4Flags::OSFXSR);
            f.insert(Cr4Flags::OSXMMEXCPT_ENABLE);
        });
    }
}

/// Adopt the currently-running code (`kernel_main`) as thread 0, the idle
/// thread.
pub fn init() {
    enable_sse();
    interrupts::without_interrupts(|| {
        let mut s = SCHED.lock();
        let mut t = Thread::new(0, "kernel_main", State::Running, 0, None);
        let now = timing::now();
        t.first_run = now;
        t.dispatched_at = now;
        s.threads.push(Box::new(t));
        s.current = 0;
    });
}

/// Create a new thread that will start running `entry(arg)` the next time the
/// scheduler picks it. Returns its id.
pub fn spawn(name: &str, entry: extern "C" fn(usize), arg: usize) -> usize {
    spawn_inner(name, entry, arg, false)
}

/// Like `spawn`, but the thread is a *system* thread: whenever it is ready it
/// runs before any ordinary thread, whatever the policy.
pub fn spawn_system(name: &str, entry: extern "C" fn(usize), arg: usize) -> usize {
    spawn_inner(name, entry, arg, true)
}

fn spawn_inner(name: &str, entry: extern "C" fn(usize), arg: usize, system: bool) -> usize {
    // Free the stacks of threads that have finished since the last spawn.
    // (Stage 35 fix: reaping used to happen only on `ps`/`kill`, so a long
    // benchmark leaked a 32 KiB stack per finished process until the heap ran
    // out.)
    reap();
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
        let mut t = Thread::new(id, name, State::Ready, rsp, Some(stack));
        t.system = system;
        t.vruntime = s.min_vruntime; // a newcomer starts level with the field
        s.threads.push(Box::new(t));
        id
    })
}

/// Record what a thread says it is. Call before it first runs (`spawn_bytes`
/// does this inside the same interrupts-off section that spawns it).
pub fn set_hint(id: usize, hint: Hint) {
    interrupts::without_interrupts(|| {
        let mut s = SCHED.lock();
        if let Some(t) = s.threads.iter_mut().find(|t| t.id == id) {
            t.hint = hint;
            t.level = if policy() == Policy::HintedMlfq { hint.level().min(mlfq_levels() - 1) } else { 0 };
        }
    });
}

/// `hint`: slices grow with level (Q << level) unless `hscale=0`.
static HINT_SCALE: AtomicBool = AtomicBool::new(true);
/// `hint`: map io-hinted threads to the top level (`iotop=1`).
static HINT_IO_TOP: AtomicBool = AtomicBool::new(false);
/// `hint`: aging lifts a waiting thread one level per period (`agestep=1`)
/// instead of straight to the top.
static HINT_AGE_STEP: AtomicBool = AtomicBool::new(false);
pub fn set_hint_age_step(on: bool) {
    HINT_AGE_STEP.store(on, Ordering::SeqCst);
}
pub fn set_hint_shape(scale: bool, io_top: bool) {
    HINT_SCALE.store(scale, Ordering::SeqCst);
    HINT_IO_TOP.store(io_top, Ordering::SeqCst);
}

fn quantum_for(pol: Policy, level: u8) -> u64 {
    let q = quantum_ms();
    match pol {
        Policy::RoundRobin => q,
        Policy::Hinted if !HINT_SCALE.load(Ordering::Relaxed) => q,
        _ => q << level,
    }
}

// --- the `fair` policy: a CFS-style virtual-runtime scheduler -------------------
//
// The baseline a reviewer would ask for: Linux's CFS (2007-2023) favours
// interactive work WITHOUT hints, purely from runtime behaviour. Mechanism (see
// the Linux kernel's Documentation/scheduler/sched-design-CFS.rst):
//   * every thread accumulates "virtual runtime" = CPU time used (all weights
//     equal here; no nice levels), and the runnable thread with the least runs;
//   * a target latency (here: the base quantum Q) is divided among the
//     runnable threads, so each gets a slice of Q / n, but at least one tick;
//   * a thread waking from sleep is placed at max(its vruntime,
//     min_vruntime - Q/2): credit for having slept, but bounded
//     ("gentle fair sleepers");
//   * a woken thread preempts the running one if it is more than
//     FAIR_WAKEUP_GRAN_MS behind it; otherwise threads switch when a slice
//     ends (or when someone falls a whole slice behind).
// Parameters are scaled to this kernel's 1 ms tick (Linux's defaults are 6 ms
// latency, 0.75 ms minimum granularity, 1 ms wake-up granularity).

pub const DEFAULT_FAIR_GRAN_MS: u64 = 1;
/// Sleeper credit as a percentage of the base quantum (Linux's "gentle fair
/// sleepers" uses half the latency target).
pub const DEFAULT_FAIR_CREDIT_PCT: u64 = 50;
static FAIR_GRAN_MS: AtomicU64 = AtomicU64::new(DEFAULT_FAIR_GRAN_MS);
/// The "preempt if a waiting thread has fallen a whole slice behind" rule. On by
/// default. Exists so an experiment can switch it off and show that the
/// wake-up granularity knob is wired up (with the rule on, it subsumes the
/// granularity rule for any sleeper whose lag exceeds one slice).
pub const DEFAULT_FAIR_SLICE_LAG: bool = true;
static FAIR_SLICE_LAG: AtomicBool = AtomicBool::new(DEFAULT_FAIR_SLICE_LAG);
pub fn set_fair_slice_lag(on: bool) {
    FAIR_SLICE_LAG.store(on, Ordering::SeqCst);
}
static FAIR_CREDIT_PCT: AtomicU64 = AtomicU64::new(DEFAULT_FAIR_CREDIT_PCT);
/// The shortest slice `fair` hands out, however many threads share the
/// target latency (Linux's minimum granularity; here whole 1 ms ticks).
pub const DEFAULT_FAIR_FLOOR_MS: u64 = 1;
static FAIR_FLOOR_MS: AtomicU64 = AtomicU64::new(DEFAULT_FAIR_FLOOR_MS);
pub fn set_fair_floor_ms(ms: u64) {
    FAIR_FLOOR_MS.store(ms.max(1), Ordering::SeqCst);
}

pub fn set_fair_gran_ms(ms: u64) {
    FAIR_GRAN_MS.store(ms, Ordering::SeqCst);
}
pub fn set_fair_credit_pct(p: u64) {
    FAIR_CREDIT_PCT.store(p.min(1000), Ordering::SeqCst);
}

fn ms_cycles(ms: u64) -> u64 {
    ms * timing::tsc_hz() / 1000
}

/// vruntime including the run in progress, if this is the running thread.
fn eff_vruntime(t: &Thread, now: u64) -> u64 {
    if t.state == State::Running {
        t.vruntime + now.saturating_sub(t.dispatched_at)
    } else {
        t.vruntime
    }
}

/// Place a thread that just woke up (sleeper credit, bounded).
fn fair_place_woken(t: &mut Thread, min_vruntime: u64) {
    let credit = ms_cycles(quantum_ms()) * FAIR_CREDIT_PCT.load(Ordering::Relaxed) / 100;
    t.vruntime = t.vruntime.max(min_vruntime.saturating_sub(credit));
}

/// Number of ordinary (non-idle, non-system) runnable threads.
fn nr_runnable(s: &Scheduler) -> u64 {
    s.threads
        .iter()
        .filter(|t| t.id != 0 && !t.system && matches!(t.state, State::Ready | State::Running))
        .count() as u64
}

/// How many times longer than the plain fair slice a thread hinted `cpu` gets
/// under `hfair`. A knob of the study (swept in experiments/plans/hfair_*.txt).
static HFAIR_MULT: AtomicU64 = AtomicU64::new(4);

pub fn hfair_mult() -> u64 {
    HFAIR_MULT.load(Ordering::Relaxed)
}
pub fn set_hfair_mult(m: u64) {
    HFAIR_MULT.store(m.clamp(1, 64), Ordering::SeqCst);
}

/// The fair slice in ticks: target latency shared among runnable threads.
fn fair_slice(s: &Scheduler) -> u64 {
    (quantum_ms() / nr_runnable(s).max(1)).max(FAIR_FLOOR_MS.load(Ordering::Relaxed))
}

/// The slice for one thread. Plain `fair`: the same for everyone. `hfair`: a
/// thread declared `cpu` gets a longer slice, so it is switched out less
/// often. That is the ONLY thing a hint does here. It never changes who is
/// picked (vruntime alone decides that), so a wrong hint can cost some
/// efficiency but cannot delay or starve another thread by more than the
/// slice-length factor, and an interactive wake-up still preempts on vruntime.
fn fair_slice_for(s: &Scheduler, t: &Thread, pol: Policy) -> u64 {
    let base = fair_slice(s);
    if pol == Policy::HintedFair && t.hint == Hint::Cpu {
        base * hfair_mult()
    } else {
        base
    }
}

/// The priority level the policy assigns this thread right now (0 = highest).
fn eff_level(t: &Thread, pol: Policy, now: u64) -> u8 {
    match pol {
        Policy::RoundRobin => 0,
        Policy::Hinted => {
            let aging = AGING_MS.load(Ordering::Relaxed) * timing::tsc_hz() / 1000;
            let waited = now.saturating_sub(t.ready_since);
            if t.state == State::Ready && aging > 0 && waited > aging {
                if HINT_AGE_STEP.load(Ordering::Relaxed) {
                    // graded aging: up one level per aging period waited
                    let periods = (waited / aging).min(u8::MAX as u64) as u8;
                    t.hint.level().saturating_sub(periods)
                } else {
                    0
                }
            } else {
                t.hint.level()
            }
        }
        Policy::Mlfq | Policy::HintedMlfq => t.level,
        Policy::Fair | Policy::HintedFair => 0,
    }
}

/// Overall rank (lower runs first): system threads above everything, then
/// the policy's own levels.
fn rank(t: &Thread, pol: Policy, now: u64) -> u8 {
    if t.system {
        0
    } else {
        1 + eff_level(t, pol, now)
    }
}

/// Choose who runs next: the best (lowest) rank among ready threads, ties
/// broken round-robin starting just after the current thread. The current
/// thread competes too if it is still runnable, but comes *last* in the
/// rotation. Thread 0 (idle) is never chosen here.
fn pick(s: &Scheduler, pol: Policy, now: u64) -> Option<usize> {
    let n = s.threads.len();
    let cur = s.current;
    let mut best: Option<((u8, u64), usize)> = None;
    for k in 1..=n {
        let i = (cur + k) % n;
        let t = &s.threads[i];
        if t.id == 0 {
            continue;
        }
        let runnable = t.state == State::Ready || (i == cur && t.state == State::Running);
        if !runnable {
            continue;
        }
        // (rank, then -- for `fair` only -- vruntime): smallest wins; exact
        // ties keep the round-robin order.
        let vr = if pol.is_fair() && !t.system { eff_vruntime(t, now) } else { 0 };
        let key = (rank(t, pol, now), vr);
        if best.map_or(true, |(b, _)| key < b) {
            best = Some((key, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Switch to whoever should run now. Interrupts must already be disabled. If
/// the current thread is still the best choice it simply keeps running with a
/// fresh time slice.
fn schedule() {
    let t0 = timing::now();
    let pol = policy();
    let old_fx: *mut FxArea;
    let (old_ptr, new_rsp, next_id) = {
        let mut s = SCHED.lock();
        let cur = s.current;
        let next = match pick(&s, pol, t0) {
            Some(i) => i,
            None if s.threads[cur].state == State::Running => {
                // Nothing else to run (current may be the idle thread).
                s.threads[cur].slice_used = 0;
                drop(s);
                SCHED_CYCLES.fetch_add(timing::now() - t0, Ordering::Relaxed);
                return;
            }
            None => 0, // current is blocked/sleeping/dead: run the idle thread
        };
        if next == cur {
            s.threads[cur].slice_used = 0;
            drop(s);
            SCHED_CYCLES.fetch_add(timing::now() - t0, Ordering::Relaxed);
            return;
        }

        {
            let c = &mut s.threads[cur];
            let ran = t0.saturating_sub(c.dispatched_at);
            c.cpu += ran;
            c.vruntime += ran;
            if c.state == State::Running {
                c.make_ready(t0, false);
                c.preempts += 1;
            }
        }
        {
            let t = &mut s.threads[next];
            let waited = t0.saturating_sub(t.ready_since);
            t.waited += waited;
            if t.woke && t.lat.len() < MAX_LAT_SAMPLES {
                t.lat.push(waited);
            }
            t.woke = false;
            if t.first_run == 0 {
                t.first_run = t0;
            }
            t.dispatched_at = t0;
            t.slice_used = 0;
            t.state = State::Running;
            t.switches_in += 1;
        }
        s.current = next;
        // Advance the fair floor to the smallest vruntime still in the race
        // (it never moves backwards).
        // (Only under `fair`: the other policies must cost exactly what they
        // did before this policy existed, or old and new results would be
        // measured on different kernels.)
        if pol.is_fair() {
            if let Some(m) = s
                .threads
                .iter()
                .filter(|t| t.id != 0 && !t.system && matches!(t.state, State::Ready | State::Running))
                .map(|t| t.vruntime)
                .min()
            {
                s.min_vruntime = s.min_vruntime.max(m);
            }
        }
        let old_ptr = &mut s.threads[cur].saved_rsp as *mut usize;
        old_fx = &mut *s.threads[cur].fx as *mut FxArea;
        (old_ptr, s.threads[next].saved_rsp, s.threads[next].id)
    }; // lock released BEFORE switching: the thread we switch to will want it
    CTX_SWITCHES.fetch_add(1, Ordering::Relaxed);
    SCHED_CYCLES.fetch_add(timing::now() - t0, Ordering::Relaxed);
    // Give the incoming thread its own address space (user processes have
    // private page tables) and kernel-entry stack before we run it.
    crate::usermode::on_switch_to(next_id);
    // The kernel itself is compiled without SSE (soft-float), so the only
    // floating-point state in the registers belongs to user programs. Save
    // ours before leaving; restore it when we are switched back to (brand-new
    // threads never return here -- they get a clean state in `enter_user`).
    unsafe {
        core::arch::asm!("fxsave64 [{}]", in(reg) old_fx, options(nostack, preserves_flags));
        switch_context(old_ptr, new_rsp);
        core::arch::asm!("fxrstor64 [{}]", in(reg) old_fx, options(nostack, preserves_flags));
    }
}

/// Called from the timer interrupt handler (interrupts already disabled, EOI
/// already sent): wake due sleepers, charge the running thread for this tick,
/// and switch if its slice is used up or someone more urgent is ready.
pub fn on_tick() {
    let now_tick = crate::interrupts::ticks();
    let now = timing::now();
    let pol = policy();
    let need = {
        let mut s = SCHED.lock();
        if s.threads.is_empty() {
            return; // timer runs from boot; the scheduler starts later
        }
        let minv = s.min_vruntime;
        for t in s.threads.iter_mut() {
            if t.state == State::Sleeping && t.wake_tick <= now_tick {
                t.make_ready(now, true);
                if pol.is_fair() {
                    fair_place_woken(t, minv);
                }
            }
        }
        if matches!(pol, Policy::Mlfq | Policy::HintedMlfq)
            && now_tick.saturating_sub(LAST_BOOST.load(Ordering::Relaxed)) >= BOOST_MS.load(Ordering::Relaxed)
        {
            LAST_BOOST.store(now_tick, Ordering::Relaxed);
            for t in s.threads.iter_mut() {
                t.level = 0;
                t.used_at_level = 0;
            }
        }
        let cur = s.current;
        let cur_state = s.threads[cur].state;
        if cur_state != State::Running {
            true
        } else if s.threads[cur].id == 0 {
            // Idle: switch as soon as anything is ready.
            s.threads.iter().any(|t| t.id != 0 && t.state == State::Ready)
        } else {
            let mut expired = false;
            {
                let c = &mut s.threads[cur];
                c.slice_used += 1;
                c.used_at_level += 1;
                if matches!(pol, Policy::Mlfq | Policy::HintedMlfq)
                    && !c.system
                    && c.used_at_level >= quantum_for(pol, c.level)
                {
                    // Used its whole allotment at this level: CPU-bound
                    // behaviour, so it sinks one level.
                    if c.level + 1 < mlfq_levels() {
                        c.level += 1;
                    }
                    c.used_at_level = 0;
                    expired = true;
                }
            }
            let my_level = eff_level(&s.threads[cur], pol, now);
            let slice = if pol.is_fair() {
                fair_slice_for(&s, &s.threads[cur], pol)
            } else {
                quantum_for(pol, my_level)
            };
            if s.threads[cur].slice_used >= slice {
                expired = true;
            }
            // `fair`: a thread woken since it last ran preempts if it is more
            // than the wake-up granularity behind; any thread preempts if it
            // has fallen a whole slice behind.
            if pol.is_fair() && !s.threads[cur].system {
                let cur_vr = eff_vruntime(&s.threads[cur], now);
                let gran = ms_cycles(FAIR_GRAN_MS.load(Ordering::Relaxed));
                let slice_c = ms_cycles(slice);
                let lag_rule = FAIR_SLICE_LAG.load(Ordering::Relaxed);
                if s.threads.iter().any(|t| {
                    t.id != 0
                        && !t.system
                        && t.state == State::Ready
                        && ((t.woke && t.vruntime + gran < cur_vr)
                            || (lag_rule && t.vruntime + slice_c < cur_vr))
                }) {
                    expired = true;
                }
            }
            // Someone more urgent ready? Under round robin only a system
            // thread counts (round robin has no priorities of its own).
            let my_rank = rank(&s.threads[cur], pol, now);
            let more_urgent = s.threads.iter().any(|t| {
                t.id != 0
                    && t.state == State::Ready
                    && (pol != Policy::RoundRobin || t.system)
                    && rank(t, pol, now) < my_rank
            });
            expired || more_urgent
        }
    };
    if need {
        schedule();
    }
}

/// Block the calling thread for `ticks` timer ticks (1 ms each) and let other
/// threads run in the meantime. A sleeping thread uses no CPU at all until its
/// wake-up tick.
pub fn sleep_ticks(ticks: u64) {
    interrupts::without_interrupts(|| {
        {
            let mut s = SCHED.lock();
            let c = s.current;
            s.threads[c].state = State::Sleeping;
            s.threads[c].wake_tick = crate::interrupts::ticks() + ticks.max(1);
            s.threads[c].blocks += 1;
        }
        schedule();
    });
}

/// Block the calling thread until someone calls `wake_all(key)`.
///
/// The caller must check its wait condition and call this **in the same
/// interrupts-disabled section** — then no wake-up can slip in between "I see
/// nothing to do" and "I'm blocked" (on one CPU, nothing else runs while
/// interrupts are off). After waking, re-check the condition: a wake-up means
/// "something changed", not "your thing is definitely there".
pub fn block_current(key: u64) {
    interrupts::without_interrupts(|| {
        {
            let mut s = SCHED.lock();
            let c = s.current;
            s.threads[c].state = State::Blocked;
            s.threads[c].block_key = key;
            s.threads[c].blocks += 1;
        }
        schedule();
    });
}

/// Make every thread blocked on `key` ready. Returns how many woke. Safe from
/// interrupt handlers.
pub fn wake_all(key: u64) -> usize {
    interrupts::without_interrupts(|| {
        let now = timing::now();
        let mut s = SCHED.lock();
        let minv = s.min_vruntime;
        let fair = policy().is_fair();
        let mut n = 0;
        for t in s.threads.iter_mut() {
            if t.state == State::Blocked && t.block_key == key {
                t.make_ready(now, true);
                if fair {
                    fair_place_woken(t, minv);
                }
                n += 1;
            }
        }
        n
    })
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

/// Mark the current thread finished and switch away for good.
pub fn exit_current() -> ! {
    interrupts::without_interrupts(|| {
        {
            let mut s = SCHED.lock();
            let c = s.current;
            s.threads[c].state = State::Dead;
        }
        schedule();
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
    pub level: u8,
    pub hint: Hint,
    pub cpu_us: u64,
}

pub fn snapshot() -> Vec<ThreadInfo> {
    reap();
    let now_tick = crate::interrupts::ticks();
    let now = timing::now();
    interrupts::without_interrupts(|| {
        let s = SCHED.lock();
        let cur = s.current;
        s.threads
            .iter()
            .enumerate()
            .map(|(i, t)| ThreadInfo {
                id: t.id,
                name: t.name.clone(),
                state: t.state,
                switches: t.switches_in,
                wake_in: if t.state == State::Sleeping {
                    t.wake_tick.saturating_sub(now_tick)
                } else {
                    0
                },
                level: t.level,
                hint: t.hint,
                cpu_us: timing::cycles_to_us(
                    t.cpu + if i == cur { now.saturating_sub(t.dispatched_at) } else { 0 },
                ),
            })
            .collect()
    })
}

/// Final measurements for one thread (all times in microseconds).
pub struct ThreadStats {
    pub id: usize,
    pub hint: Hint,
    /// Created -> first ran.
    pub response_us: u64,
    /// Created -> now (call this when the work is done).
    pub turnaround_us: u64,
    pub cpu_us: u64,
    /// Total time ready but not running.
    pub wait_us: u64,
    pub switches: u64,
    pub preempts: u64,
    pub blocks: u64,
    /// Wake-up latencies, sorted ascending.
    pub lat_us: Vec<u64>,
}

/// Snapshot the calling thread's statistics, up to this moment.
pub fn current_stats() -> ThreadStats {
    let now = timing::now();
    let (id, hint, created, first, cpu, dispatched, waited, sw, pre, blk, lat) =
        interrupts::without_interrupts(|| {
            let s = SCHED.lock();
            let t = &s.threads[s.current];
            (
                t.id,
                t.hint,
                t.created,
                t.first_run,
                t.cpu,
                t.dispatched_at,
                t.waited,
                t.switches_in,
                t.preempts,
                t.blocks,
                t.lat.clone(),
            )
        });
    let mut lat_us: Vec<u64> = lat.into_iter().map(timing::cycles_to_us).collect();
    lat_us.sort_unstable();
    ThreadStats {
        id,
        hint,
        response_us: timing::cycles_to_us(first.saturating_sub(created)),
        turnaround_us: timing::cycles_to_us(now.saturating_sub(created)),
        cpu_us: timing::cycles_to_us(cpu + now.saturating_sub(dispatched)),
        wait_us: timing::cycles_to_us(waited),
        switches: sw,
        preempts: pre,
        blocks: blk,
        lat_us,
    }
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

/// A well-behaved background thread: sleeps 1 second, wakes, logs, repeats.
/// While it sleeps it costs nothing.
pub extern "C" fn ticker_entry(_: usize) {
    let id = current_id();
    loop {
        sleep_ticks(crate::interrupts::HZ);
        bump_progress();
        let n = progress(id);
        crate::println!("[ticker {}] woke up, beat #{} (tick {})", id, n, crate::interrupts::ticks());
        crate::serial_println!("[ticker {}] woke up, beat #{} (tick {})", id, n, crate::interrupts::ticks());
    }
}
