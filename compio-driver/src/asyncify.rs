use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};

type BoxClosure = Box<dyn FnOnce() + Send>;

struct CounterGuard(Arc<AtomicUsize>);

impl Drop for CounterGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn worker(
    receiver: Receiver<BoxClosure>,
    counter: Arc<AtomicUsize>,
    timeout: Duration,
) -> impl FnOnce() {
    move || {
        counter.fetch_add(1, Ordering::AcqRel);
        let _guard = CounterGuard(counter);
        while let Ok(f) = receiver.recv_timeout(timeout) {
            f();
        }
    }
}

/// A thread pool to perform blocking operations in other threads.
///
/// ## Platform specific
/// * io-uring: the driver doesn't user this thread pool.
pub struct AsyncifyPool {
    sender: Sender<BoxClosure>,
    receiver: Receiver<BoxClosure>,
    counter: Arc<AtomicUsize>,
    thread_limit: usize,
    recv_timeout: Duration,
}

impl AsyncifyPool {
    /// Create [`AsyncifyPool`] with thread number limit and channel receive
    /// timeout.
    pub fn new(thread_limit: usize, recv_timeout: Duration) -> Self {
        let (sender, receiver) = bounded(0);
        Self {
            sender,
            receiver,
            counter: Arc::new(AtomicUsize::new(0)),
            thread_limit,
            recv_timeout,
        }
    }

    /// Send a closure to another thread.
    pub fn dispatch(&self, f: impl FnOnce() + Send + 'static) -> bool {
        match self.sender.try_send(Box::new(f) as BoxClosure) {
            Ok(_) => true,
            Err(e) => match e {
                TrySendError::Full(f) => {
                    if self.counter.load(Ordering::Acquire) >= self.thread_limit {
                        false
                    } else {
                        std::thread::spawn(worker(
                            self.receiver.clone(),
                            self.counter.clone(),
                            self.recv_timeout,
                        ));
                        self.sender.send(f).expect("the channel should not be full");
                        true
                    }
                }
                TrySendError::Disconnected(_) => {
                    unreachable!("receiver should not all disconnected")
                }
            },
        }
    }
}
