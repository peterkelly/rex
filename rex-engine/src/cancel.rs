#![forbid(unsafe_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::task::{Context, Poll};

#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    cancelled: AtomicBool,
    wakers: Mutex<Vec<std::task::Waker>>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            wakers: Mutex::new(Vec::new()),
        }
    }
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        let wakers = std::mem::take(&mut *self.inner.wakers.lock().expect("poisoned cancel wakers"));
        for w in wakers {
            w.wake();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    pub fn cancelled(&self) -> Cancelled {
        Cancelled { token: self.clone() }
    }
}

pub struct Cancelled {
    token: CancellationToken,
}

impl Future for Cancelled {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.token.is_cancelled() {
            return Poll::Ready(());
        }
        let waker = cx.waker().clone();
        let mut wakers = self
            .token
            .inner
            .wakers
            .lock()
            .expect("poisoned cancel wakers");
        if !wakers.iter().any(|w| w.will_wake(&waker)) {
            wakers.push(waker);
        }
        if self.token.is_cancelled() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}
