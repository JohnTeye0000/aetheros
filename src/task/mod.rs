//! Cooperative multitasking with async/await.
//!
//! A `Future` in Rust is a computation that can be *polled*: each poll makes a
//! little progress and returns either `Ready(value)` (done) or `Pending` (not
//! yet — try again later). An `async fn` compiles into exactly such a future.
//!
//! That's all we need for cooperative multitasking on a single CPU: wrap each
//! future in a `Task`, hand the tasks to an executor, and have the executor
//! poll them in turn. When a task hits an `.await` that isn't ready, it returns
//! `Pending` — voluntarily handing the CPU to the next task. No threads, no
//! preemption; tasks *cooperate* by yielding.

use alloc::boxed::Box;
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

pub mod executor;

/// A unit of work: a heap-allocated, pinned future that resolves to `()`.
///
/// It's `Pin<Box<...>>` because a future may hold references into itself, so
/// once we start polling it, it must never move in memory. `Box::pin` puts it
/// on the heap and promises exactly that.
pub struct Task {
    future: Pin<Box<dyn Future<Output = ()>>>,
}

impl Task {
    /// Wrap any `()`-returning future (e.g. the result of an `async fn`) as a Task.
    pub fn new(future: impl Future<Output = ()> + 'static) -> Task {
        Task {
            future: Box::pin(future),
        }
    }

    /// Make one step of progress. The executor calls this.
    fn poll(&mut self, context: &mut Context) -> Poll<()> {
        self.future.as_mut().poll(context)
    }
}

/// A future that yields control back to the executor exactly once.
///
/// The first poll returns `Pending` (so the executor moves on to other tasks);
/// the next poll returns `Ready`. Sprinkling `yield_now().await` through a task
/// is how it cooperatively gives other tasks a turn.
pub struct YieldNow {
    yielded: bool,
}

/// Create a [`YieldNow`] future — `yield_now().await` hands the CPU to the next task.
pub fn yield_now() -> YieldNow {
    YieldNow { yielded: false }
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<()> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            // Ask to be polled again. (Our round-robin executor re-queues
            // Pending tasks regardless, so this is belt-and-suspenders.)
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}
