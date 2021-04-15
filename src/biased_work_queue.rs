use crossbeam::channel;
use crossbeam::sync::{Parker, Unparker};

pub enum QueueError {
    Disconnected,
}

pub trait QueueReceiver: Clone {
    type Item;
    fn try_receive(&self) -> Result<Option<Self::Item>, QueueError>;
    fn receive(&self) -> Result<Self::Item, QueueError>;
}

impl<T> QueueReceiver for channel::Receiver<T> {
    type Item = T;
    fn try_receive(&self) -> Result<Option<Self::Item>, QueueError> {
        match self.try_recv() {
            Ok(v) => Ok(Some(v)),
            Err(channel::TryRecvError::Empty) => Ok(None),
            Err(channel::TryRecvError::Disconnected) => Err(QueueError::Disconnected),
        }
    }
    fn receive(&self) -> Result<Self::Item, QueueError> {
        self.recv().map_err(|_| QueueError::Disconnected)
    }
}

pub struct BiasedWorkQueueReceiver<R: QueueReceiver> {
    /// The single work queue channel
    receiver: R,
    /// For all threads but the first one, we block on this instead of the channel.
    parker: Option<Parker>,
    /// For all threads but the last one, we wake up the next thread
    /// when there is more than enough work for this thread.
    next_thread_unparker: Option<Unparker>,
}

/// Like a regular unbounded (no backpressure) queue but we prefer to send work to
/// low-numbered threads, keeping high-numbered threads idle as much as possible.
/// Basically there are two regimes: we're keeping up and the queue is short,
/// or we're not keeping up and the queue is growing.
/// We return one receiver object per thread that will consume work.
pub fn biased_work_queue<R: QueueReceiver>(
    rx: R,
    threads: usize,
) -> Vec<BiasedWorkQueueReceiver<R>> {
    assert!(threads > 0);
    let mut receivers = Vec::with_capacity(threads);
    let mut parker = None;
    for i in 0..threads {
        let next_thread_parker = if i + 1 < threads {
            Some(Parker::new())
        } else {
            None
        };
        receivers.push(BiasedWorkQueueReceiver {
            receiver: rx.clone(),
            parker: parker.take(),
            next_thread_unparker: next_thread_parker.as_ref().map(|v| v.unparker().clone()),
        });
        parker = next_thread_parker;
    }
    receivers
}

const BATCH_SIZE: usize = 20;

impl<R: QueueReceiver> BiasedWorkQueueReceiver<R> {
    /// Returns a list of items to work on.
    pub fn recv(&self) -> Result<Vec<R::Item>, QueueError> {
        let mut buf = Vec::new();
        loop {
            // Grab up to BATCH_SIZE work items without blocking.
            while buf.len() < BATCH_SIZE {
                match self.receiver.try_receive() {
                    Ok(Some(v)) => buf.push(v),
                    Ok(None) => break,
                    Err(QueueError::Disconnected) => {
                        if buf.is_empty() {
                            if let Some(unparker) = self.next_thread_unparker.as_ref() {
                                unparker.unpark();
                            }
                            return Err(QueueError::Disconnected);
                        }
                        return Ok(buf);
                    }
                }
            }
            if buf.is_empty() {
                // We need to block
                if let Some(parker) = self.parker.as_ref() {
                    // Wait for the previous thread to wake us up because it's overloaded.
                    parker.park();
                } else {
                    // This is the first thread, so just do a blocking read from the queue.
                    if let Ok(v) = self.receiver.receive() {
                        buf.push(v);
                    }
                    // Continue and grab as much work as we can without blocking again.
                    // If we disconnected then we'll notice that above and exit.
                }
            } else {
                if buf.len() >= BATCH_SIZE {
                    // There seems to be enough work to wake up the next thread.
                    if let Some(unparker) = self.next_thread_unparker.as_ref() {
                        unparker.unpark();
                    }
                }
                return Ok(buf);
            }
        }
    }
}
