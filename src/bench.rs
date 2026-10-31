//! The benchmark harness (Stage 35): turns the kernel into an experiment.
//!
//! `bench <workload> [policy] [q=N] [hints=on|none|wrong] [seed=N]` sets the
//! scheduling policy, launches a fixed mix of benchmark processes at (nearly)
//! the same moment, blocks until every one has exited, then prints one
//! machine-readable line per process plus a summary line, on the serial port:
//!
//! ```text
//! RESULT,run,workload,policy,q,hints,seed,pid,prog,hint,status,response_us,
//!        turnaround_us,cpu_us,wait_us,switches,preempts,blocks,
//!        lat_n,lat_p50,lat_p95,lat_p99,lat_max
//! SUMMARY,run,workload,policy,q,hints,seed,nprocs,makespan_us,ctx_switches,sched_us
//! LAT,run,pid,<every wake-up latency sample in us, ';'-separated>
//! ```
//!
//! The host-side scripts in `experiments/` parse exactly this.
//!
//! **Hint modes** make the hint itself an experimental variable: `on` gives
//! each process its true hint, `none` gives every process no hint (so a
//! hint-based policy degenerates), `wrong` deliberately lies (CPU-bound work
//! claims to be interactive and vice versa) to measure what bad information
//! costs.
//!
//! **Seeds**: with `seed=0` every process is spawned back-to-back. A non-zero
//! seed inserts a pseudo-random 0-3 ms gap between launches, so repeated runs
//! see slightly different arrival patterns (otherwise, under `-icount`, every
//! repeat would be bit-for-bit identical and "variance" would be meaningless).
//!
//! **Autorun**: if the filesystem holds a file named `autorun` at boot, its
//! lines are executed by a kernel thread (`bench ...`, `repeat N bench ...`,
//! `echo ...`, `poweroff`), so a whole experiment can run with no keyboard.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

use crate::scheduler::{self, Hint, Policy, ThreadStats};
use crate::{println, serial_print, serial_println};

const KEY_BENCH: u64 = 0x5000_0000_0000_0001;

static QUIET: AtomicBool = AtomicBool::new(false);
static AUTORUN: AtomicBool = AtomicBool::new(false);

/// True while an unattended `autorun` script is executing. The panic handler
/// uses this to power the machine off instead of spinning forever, so a
/// crashed experiment ends immediately (and visibly) rather than looking like
/// a hang.
pub fn autorun_active() -> bool {
    AUTORUN.load(Ordering::Relaxed)
}
static ACTIVE: AtomicBool = AtomicBool::new(false);
static RUN_ID: AtomicU64 = AtomicU64::new(0);

struct Rec {
    name: String,
    status: u64,
    st: ThreadStats,
}

static RESULTS: Mutex<Vec<Rec>> = Mutex::new(Vec::new());

/// While a benchmark runs, user processes don't print launch/exit banners
/// (printing to the VGA console is slow and would distort the measurements).
pub fn quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

/// Called by every user process's thread when its program finishes.
pub fn record(name: &str, status: u64, st: ThreadStats) {
    if !ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    without_interrupts(|| {
        RESULTS.lock().push(Rec { name: String::from(name), status, st });
    });
    scheduler::wake_all(KEY_BENCH);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HintMode {
    On,
    None,
    Wrong,
}

impl HintMode {
    fn name(self) -> &'static str {
        match self {
            HintMode::On => "on",
            HintMode::None => "none",
            HintMode::Wrong => "wrong",
        }
    }
    fn apply(self, truth: Hint) -> Hint {
        match self {
            HintMode::On => truth,
            HintMode::None => Hint::None,
            HintMode::Wrong => match truth {
                Hint::Cpu => Hint::Interactive,
                Hint::Interactive => Hint::Cpu,
                Hint::Io => Hint::Cpu,
                Hint::None => Hint::None,
            },
        }
    }
}

/// What a `bench` line means when it doesn't say.
const DEFAULT_QUANTUM_MS: u64 = 10;
const DEFAULT_HFAIR_MULT: u64 = 4;

pub struct Config {
    pub workload: String,
    pub policy: Policy,
    pub quantum: u64,
    pub hints: HintMode,
    pub seed: u64,
    /// `hfair` slice multiplier for cpu-hinted threads.
    pub mult: u64,
    /// `hint`: wait (ms) after which a thread is aged to top priority.
    pub aging: u64,
    /// `mlfq`/`hmlfq`: period (ms) of the boost that returns everyone to the top level.
    pub boost: u64,
    /// `fair`/`hfair`: wake-up preemption granularity (ms).
    pub gran: u64,
    /// `fair`/`hfair`: sleeper credit, percent of the base quantum.
    pub credit: u64,
    /// `fair`/`hfair`: the slice-lag preemption rule (default on; `lag=0` turns it off).
    pub lag: bool,
}

pub const WORKLOADS: &str = "cpu4  mixed  interactive  all  scale<N>  mix:C,W,I  mix2:C,W,I  fpu  files:a.axe+b.axe+...";

/// One process to launch: (name, machine code, the hint it declares).
type Launch = (String, Vec<u8>, Hint);

/// Resolve a workload name to the processes it launches. Besides the built-in
/// mixes, `files:a.axe+b.axe+...` launches program files from the filesystem
/// (e.g. PlainLang programs compiled by plainaxe), each with the hint written
/// in its own header -- that's how compiler-derived hints enter the kernel.
fn load_workload(workload: &str) -> Result<Vec<Launch>, &'static str> {
    if let Some(list) = workload.strip_prefix("files:") {
        let mut v = Vec::new();
        for f in list.split('+').filter(|f| !f.is_empty()) {
            let text = crate::fs::read(f).ok_or("no such file")?;
            let (code, hint) = crate::usermode::parse_axe_full(&text)?;
            v.push((String::from(f), code, hint));
        }
        if v.is_empty() {
            return Err("empty files: list");
        }
        return Ok(v);
    }
    let names = programs(workload).ok_or("unknown workload")?;
    Ok(names
        .into_iter()
        .map(|p| {
            (
                String::from(p),
                crate::usermode::builtin(p).unwrap().to_vec(),
                crate::usermode::builtin_hint(p),
            )
        })
        .collect())
}

fn programs(workload: &str) -> Option<Vec<&'static str>> {
    let v = match workload {
        "cpu4" => alloc::vec!["wcpu", "wcpu", "wcpu", "wcpu"],
        "mixed" => alloc::vec!["wcpu", "wcpu", "wio", "wio"],
        "interactive" => alloc::vec!["wcpu", "wcpu", "wcpu", "wint"],
        "all" => alloc::vec!["wcpu", "wcpu", "wio", "wint"],
        // correctness check, not a performance workload: 3 processes keeping
        // values in SSE registers while being switched (exit status = corruptions)
        "fpu" => alloc::vec!["fpu", "fpu", "fpu", "wcpu"],
        // mix:C,W,I = C cpu-bound + W io-style + I interactive processes (total <= 14).
        // Lets the study draw workloads it didn't tune against.
        // mix2:C,W,I is the same but the I interactive processes have different
        // periods (15, 13, 17 ms, cycling) so they don't wake in lockstep.
        w if w.starts_with("mix:") || w.starts_with("mix2:") => {
            let staggered = w.starts_with("mix2:");
            let body = if staggered { &w["mix2:".len()..] } else { &w["mix:".len()..] };
            let mut it = body.split(',').map(|x| x.parse::<usize>().ok());
            let (c, i_o, n_i) = (it.next()??, it.next()??, it.next()??);
            if it.next().is_some() || c + i_o + n_i == 0 || c + i_o + n_i > 14 {
                return None;
            }
            let mut v = alloc::vec!["wcpu"; c];
            v.extend(core::iter::repeat("wio").take(i_o));
            for k in 0..n_i {
                v.push(if staggered { ["wint", "wint2", "wint3"][k % 3] } else { "wint" });
            }
            v
        }
        w => {
            let n: usize = w.strip_prefix("scale")?.parse().ok()?;
            if n == 0 || n > 14 {
                return None;
            }
            let mut v = alloc::vec!["wcpu"; n];
            v.push("wint");
            v
        }
    };
    Some(v)
}

pub fn parse(args: &str) -> Result<Config, &'static str> {
    let mut it = args.split_whitespace();
    let workload = it.next().ok_or("usage: bench <workload> [policy] [q=N] [hints=on|none|wrong] [seed=N]")?;
    if load_workload(workload).is_err() {
        return Err("unknown workload, or a files: program that can't be loaded");
    }
    let mut cfg = Config {
        workload: String::from(workload),
        // Defaults are CONSTANTS, not "whatever the previous run left behind":
        // a benchmark line must mean the same thing wherever it appears in a
        // plan. (Before this fix `bench` inherited the last run's quantum, so a
        // plan line without q= silently ran at the quantum of the line above it.)
        policy: Policy::RoundRobin,
        quantum: DEFAULT_QUANTUM_MS,
        hints: HintMode::On,
        seed: 0,
        mult: DEFAULT_HFAIR_MULT,
        aging: scheduler::DEFAULT_AGING_MS,
        boost: scheduler::DEFAULT_BOOST_MS,
        gran: scheduler::DEFAULT_FAIR_GRAN_MS,
        credit: scheduler::DEFAULT_FAIR_CREDIT_PCT,
        lag: scheduler::DEFAULT_FAIR_SLICE_LAG,
    };
    for tok in it {
        // `hfair8` = hfair with an 8x slice multiplier; plain `hfair` = default.
        if let Some(n) = tok.strip_prefix("hfair").filter(|n| !n.is_empty()) {
            cfg.policy = Policy::HintedFair;
            cfg.mult = n.parse().map_err(|_| "bad hfair multiplier (hfair8)")?;
        } else if let Some(p) = Policy::parse(tok) {
            cfg.policy = p;
        } else if let Some(v) = tok.strip_prefix("q=") {
            cfg.quantum = v.parse().map_err(|_| "bad q=")?;
        } else if let Some(v) = tok.strip_prefix("aging=") {
            cfg.aging = v.parse().map_err(|_| "bad aging=")?;
        } else if let Some(v) = tok.strip_prefix("gran=") {
            cfg.gran = v.parse().map_err(|_| "bad gran=")?;
        } else if let Some(v) = tok.strip_prefix("lag=") {
            cfg.lag = v != "0";
        } else if let Some(v) = tok.strip_prefix("credit=") {
            cfg.credit = v.parse().map_err(|_| "bad credit=")?;
        } else if let Some(v) = tok.strip_prefix("boost=") {
            cfg.boost = v.parse().map_err(|_| "bad boost=")?;
        } else if let Some(v) = tok.strip_prefix("seed=") {
            cfg.seed = v.parse().map_err(|_| "bad seed=")?;
        } else if let Some(v) = tok.strip_prefix("hints=") {
            cfg.hints = match v {
                "on" => HintMode::On,
                "none" => HintMode::None,
                "wrong" => HintMode::Wrong,
                _ => return Err("hints= must be on, none or wrong"),
            };
        } else {
            return Err("unknown option (policy: rr hint mlfq hmlfq fair; q=N hints=.. seed=N)");
        }
    }
    Ok(cfg)
}

fn percentile(sorted: &[u64], p: u64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    // nearest-rank: the smallest value with at least p% of samples <= it
    let rank = ((p * sorted.len() as u64) + 99) / 100;
    sorted[(rank.max(1) as usize) - 1]
}

/// Run one benchmark to completion. Must be called from a kernel thread (it
/// blocks), never from an interrupt handler.
pub fn run(cfg: &Config) {
    let progs = match load_workload(&cfg.workload) {
        Ok(p) => p,
        Err(e) => {
            serial_println!("bench: {}", e);
            return;
        }
    };
    let run_id = RUN_ID.fetch_add(1, Ordering::SeqCst) + 1;

    scheduler::set_policy(cfg.policy);
    scheduler::set_hfair_mult(cfg.mult);
    scheduler::set_aging_ms(cfg.aging);
    scheduler::set_boost_ms(cfg.boost);
    scheduler::set_fair_gran_ms(cfg.gran);
    scheduler::set_fair_credit_pct(cfg.credit);
    scheduler::set_fair_slice_lag(cfg.lag);
    scheduler::set_quantum_ms(cfg.quantum);
    without_interrupts(|| RESULTS.lock().clear());
    QUIET.store(true, Ordering::SeqCst);
    ACTIVE.store(true, Ordering::SeqCst);
    scheduler::reset_counters();

    let mut rng = cfg.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let t0 = crate::timing::now();
    let mut launched = 0usize;
    for (i, p) in progs.iter().enumerate() {
        if cfg.seed != 0 && i > 0 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let gap = rng % 4;
            if gap > 0 {
                scheduler::sleep_ticks(gap);
            }
        }
        let (name, code, declared) = p;
        let hint = cfg.hints.apply(*declared);
        match crate::usermode::spawn_bytes(name, code.clone(), "", 0, hint) {
            Ok(_) => launched += 1,
            Err(e) => {
                println!("bench: could not launch {}: {}", name, e);
                serial_println!("bench: could not launch {}: {}", name, e);
            }
        }
    }

    // Wait for every launched process: check-then-block in one
    // interrupts-off section, so the last exit can't be missed.
    loop {
        let done = without_interrupts(|| {
            if RESULTS.lock().len() >= launched {
                true
            } else {
                scheduler::block_current(KEY_BENCH);
                false
            }
        });
        if done {
            break;
        }
    }
    let makespan = crate::timing::cycles_to_us(crate::timing::now() - t0);
    let switches = scheduler::ctx_switches();
    let sched_us = crate::timing::cycles_to_us(scheduler::sched_cycles());
    ACTIVE.store(false, Ordering::SeqCst);
    QUIET.store(false, Ordering::SeqCst);

    let results: Vec<Rec> = without_interrupts(|| core::mem::take(&mut *RESULTS.lock()));
    // The multiplier is part of the configuration's identity, so it goes in the
    // policy label: hfair (default 4), hfair2, hfair8, ...
    let mut label = if cfg.policy == Policy::HintedFair && cfg.mult != 4 {
        alloc::format!("hfair{}", cfg.mult)
    } else {
        String::from(cfg.policy.name())
    };
    // Non-default aging / boost periods are part of the configuration's identity.
    if cfg.policy == Policy::Hinted && cfg.aging != scheduler::DEFAULT_AGING_MS {
        label = alloc::format!("{}-aging{}", label, cfg.aging);
    }
    if cfg.policy.is_fair() {
        if cfg.gran != scheduler::DEFAULT_FAIR_GRAN_MS {
            label = alloc::format!("{}-gran{}", label, cfg.gran);
        }
        if cfg.lag != scheduler::DEFAULT_FAIR_SLICE_LAG {
            label = alloc::format!("{}-nolag", label);
        }
        if cfg.credit != scheduler::DEFAULT_FAIR_CREDIT_PCT {
            label = alloc::format!("{}-credit{}", label, cfg.credit);
        }
    }
    if matches!(cfg.policy, Policy::Mlfq | Policy::HintedMlfq) && cfg.boost != scheduler::DEFAULT_BOOST_MS {
        label = alloc::format!("{}-boost{}", label, cfg.boost);
    }
    // Workload names may contain commas (mix:4,3,3), which would break the CSV
    // lines; the host sees them with ';' instead (mix:4;3;3).
    let workload_label = cfg.workload.replace(',', ";");
    let tag = alloc::format!(
        "{},{},{},{},{},{}",
        run_id,
        workload_label,
        label,
        cfg.quantum,
        cfg.hints.name(),
        cfg.seed
    );
    println!(
        "bench #{} {} policy={} q={} hints={} seed={}",
        run_id, workload_label, label, cfg.quantum, cfg.hints.name(), cfg.seed
    );
    println!("  pid prog  hint         turnaround_us  cpu_us   wait_us  lat_p95");
    for r in &results {
        let s = &r.st;
        serial_println!(
            "RESULT,{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            tag,
            s.id,
            r.name,
            s.hint.name(),
            r.status,
            s.response_us,
            s.turnaround_us,
            s.cpu_us,
            s.wait_us,
            s.switches,
            s.preempts,
            s.blocks,
            s.lat_us.len(),
            percentile(&s.lat_us, 50),
            percentile(&s.lat_us, 95),
            percentile(&s.lat_us, 99),
            s.lat_us.last().copied().unwrap_or(0)
        );
        println!(
            "  {:>3} {:<5} {:<12} {:>13} {:>7} {:>9} {:>8}",
            s.id,
            r.name,
            s.hint.name(),
            s.turnaround_us,
            s.cpu_us,
            s.wait_us,
            percentile(&s.lat_us, 95)
        );
    }
    for r in &results {
        if r.st.lat_us.is_empty() {
            continue;
        }
        serial_print!("LAT,{},{},", run_id, r.st.id);
        for (i, v) in r.st.lat_us.iter().enumerate() {
            if i > 0 {
                serial_print!(";");
            }
            serial_print!("{}", v);
        }
        serial_println!("");
    }
    serial_println!(
        "SUMMARY,{},{},{},{},{}",
        tag,
        results.len(),
        makespan,
        switches,
        sched_us
    );
    let (heap_used, _, live) = crate::allocator::heap_stats();
    serial_println!(
        "HEAP,{},{},{},{}",
        run_id,
        heap_used,
        crate::allocator::largest_free_block(),
        live
    );
    println!(
        "  makespan {} us, {} context switches, {} us in scheduler",
        makespan, switches, sched_us
    );
}

// --- shell + autorun entry points ------------------------------------------------

static PENDING: Mutex<Option<Config>> = Mutex::new(None);

extern "C" fn bench_thread(_: usize) {
    let cfg = without_interrupts(|| PENDING.lock().take());
    if let Some(cfg) = cfg {
        run(&cfg);
    }
    crate::shell::prompt();
}

/// Start a benchmark from the shell (which runs in interrupt context and so
/// can't block): it runs on its own kernel thread.
pub fn start_from_shell(args: &str) -> Result<(), &'static str> {
    if ACTIVE.load(Ordering::SeqCst) {
        return Err("a benchmark is already running");
    }
    let cfg = parse(args)?;
    without_interrupts(|| *PENDING.lock() = Some(cfg));
    scheduler::spawn_system("bench", bench_thread, 0);
    Ok(())
}

/// Exit QEMU (isa-debug-exit device if present, else ACPI power-off on the
/// PIIX4 power-management port QEMU's default machine provides).
pub fn poweroff() -> ! {
    use x86_64::instructions::port::Port;
    serial_println!("POWEROFF");
    unsafe {
        Port::<u32>::new(0xf4).write(0);
        Port::<u16>::new(0x604).write(0x2000);
    }
    loop {
        x86_64::instructions::hlt();
    }
}

extern "C" fn autorun_thread(_: usize) {
    let script = crate::fs::read("autorun").unwrap_or_default();
    AUTORUN.store(true, Ordering::SeqCst);
    // One-time CPU setup (NX/SMEP/SMAP, SYSCALL MSRs) happens here, before
    // the first benchmark, so no run pays for it.
    crate::usermode::init_once();
    serial_println!("
AUTORUN,start");
    for line in script.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("bench ") {
            match parse(rest) {
                Ok(cfg) => run(&cfg),
                Err(e) => { serial_println!("AUTORUN,error,{},{}", line, e); }
            }
        } else if let Some(rest) = line.strip_prefix("repeat ") {
            // repeat N bench <args> : runs with seed=1..N
            let (n, rest) = rest.split_once(' ').unwrap_or(("0", ""));
            let n: u64 = n.parse().unwrap_or(0);
            let args = rest.strip_prefix("bench ").unwrap_or(rest);
            for seed in 1..=n {
                let full = alloc::format!("{} seed={}", args, seed);
                match parse(&full) {
                    Ok(cfg) => run(&cfg),
                    Err(e) => {
                        serial_println!("AUTORUN,error,{},{}", line, e);
                        break;
                    }
                }
            }
        } else if let Some(rest) = line.strip_prefix("exec ") {
            let (file, args) = rest.split_once(' ').unwrap_or((rest, ""));
            exec_file(file.trim(), args.trim());
        } else if let Some(rest) = line.strip_prefix("echo ") {
            serial_println!("{}", rest);
        } else if line == "poweroff" {
            serial_println!("AUTORUN,done");
            poweroff();
        } else {
            serial_println!("AUTORUN,error,{},unknown command", line);
        }
    }
    serial_println!("AUTORUN,done");
    crate::shell::prompt();
}

/// Run one program file to completion, bracketing its output with markers a
/// test harness can split on: `PROG,<file>,begin` ... `PROG,<file>,end,<status>`.
fn exec_file(file: &str, args: &str) {
    let parsed = crate::fs::read(file)
        .ok_or("no such file")
        .and_then(|t| crate::usermode::parse_axe_full(&t));
    serial_println!("PROG,{},begin", file);
    match parsed {
        Err(e) => { serial_println!("\nPROG,{},end,error:{}", file, e); }
        Ok((code, hint)) => {
            QUIET.store(true, Ordering::SeqCst);
            let me = scheduler::current_id();
            match crate::usermode::spawn_bytes(file, code, args, me, hint) {
                Err(e) => { serial_println!("\nPROG,{},end,error:{}", file, e); }
                Ok(pid) => {
                    let status = crate::usermode::wait_child(pid);
                    serial_println!("\nPROG,{},end,{}", file, status as i64);
                }
            }
            QUIET.store(false, Ordering::SeqCst);
        }
    }
}

/// Called once at boot, after the filesystem is restored from the data disk.
pub fn autorun_start() {
    if crate::fs::read("autorun").is_some() {
        crate::println!("[ bench ] 'autorun' file found - running it");
        scheduler::spawn_system("autorun", autorun_thread, 0);
    }
}
