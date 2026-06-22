//! A tiny round-robin cooperative executor.
//!
//! It keeps a queue of tasks. `run` pops the front task, polls it once, and:
//!   - if it's `Ready`, drops it (the task is finished);
//!   - if it's `Pending`, pushes it to the back of the queue.
//! It keeps going until the queue is empty. Because each task yields after a
//! step, the executor naturally interleaves them — that's the multitasking.

use super::Task;
use alloc::collections::VecDeque;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

pub struct Executor {
    tasks: VecDeque<Task>,
}

impl Executor {
    pub fn new() -> Self {
        Executor {
            tasks: VecDeque::new(),
        }
    }

    /// Add a task to the run queue.
    pub fn spawn(&mut self, task: Task) {
        self.tasks.push_back(task);
    }

    /// Run every queued task to completion, round-robin.
    pub fn run(&mut self) {
        let waker = dummy_waker();
        let mut context = Context::from_waker(&waker);

        while let Some(mut task) = self.tasks.pop_front() {
            match task.poll(&mut context) {
                Poll::Ready(()) => {} // finished — drop it
                Poll::Pending => self.tasks.push_back(task), // not done — requeue
            }
        }
    }
}

// --- A no-op waker -------------------------------------------------------
//
// A real waker lets a blocked task signal "I'm ready again" so the executor can
// re-schedule it (instead of busy-polling). Our executor re-polls every Pending
// task each round anyway, so we just need a waker that does nothing. Building
// one means filling in a vtable of four function pointers.

fn dummy_raw_waker() -> RawWaker {
    fn no_op(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        dummy_raw_waker()
    }

    let vtable = &RawWakerVTable::new(clone, no_op, no_op, no_op);
    RawWaker::new(core::ptr::null(), vtable)
}

fn dummy_waker() -> Waker {
    // Safe because our vtable functions do nothing and never dereference the
    // (null) data pointer.
    unsafe { Waker::from_raw(dummy_raw_waker()) }
}
