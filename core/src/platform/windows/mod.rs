//! Windows backend: the WinRT notification listener (Phase 4) and the Win32
//! context signals that feed rule conditions.

pub mod access;
pub mod context;
pub mod listener;

/// The only module in this crate allowed to use `unsafe`: raw Win32 calls
/// that the `windows` crate exposes as `unsafe fn`. Every wrapper in here
/// keeps the blast radius to the documented, race-free call itself and is
/// exercised by headless CI so a null handle degrades instead of panicking.
#[allow(unsafe_code)]
pub mod win32;

use std::future::{Future, IntoFuture};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

/// How long to wait between polls while blocking on a WinRT async operation.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Minimal thread-parking executor.
///
/// `windows` 0.61+ projects async WinRT operations as `IntoFuture` rather
/// than exposing a blocking `get()`, and pulling in a runtime for a single
/// call would be wasteful. Completion callbacks unpark this thread.
///
/// Must not be called from a WinRT UI thread — callers run it from worker
/// threads.
pub(super) fn block_on<F>(future: F) -> F::Output
where
    F: IntoFuture,
    F::IntoFuture: Future,
{
    struct ThreadWaker(Thread);

    impl Wake for ThreadWaker {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }

    let mut future = Box::pin(future.into_future());
    let waker = Waker::from(std::sync::Arc::new(ThreadWaker(thread::current())));
    let mut context = Context::from_waker(&waker);

    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park_timeout(POLL_INTERVAL),
        }
    }
}
