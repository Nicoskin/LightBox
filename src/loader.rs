//! Background decoding: a small worker pool fed by a priority list that the
//! UI replaces wholesale on every navigation. Requests not yet started are
//! thereby cancelled, so fast browsing never builds up a backlog; decodes
//! already running finish and still land in the cache.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use crate::cache;
use crate::decode::{self, DecodedImage, SizeHint};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// Reduced (DCT-scaled) image sized for the screen.
    Preview,
    /// Full resolution, for zooming in.
    Full,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub path: PathBuf,
    pub quality: Quality,
}

pub struct Done {
    pub path: PathBuf,
    pub result: Result<Arc<DecodedImage>, String>,
}

struct State {
    queue: VecDeque<Request>,
    /// Cache keys + quality of decodes in progress.
    running: Vec<(String, Quality)>,
    hint: SizeHint,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

pub struct Loader {
    shared: Arc<Shared>,
    results: Receiver<Done>,
}

impl Loader {
    /// Starts `threads` workers. `notify` is called (from a worker) after
    /// each finished decode, to wake the UI thread.
    pub fn new(threads: usize, hint: SizeHint, notify: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                running: Vec::new(),
                hint,
                shutdown: false,
            }),
            wake: Condvar::new(),
        });
        let (tx, results) = channel();
        let notify = Arc::new(notify);
        for i in 0..threads.max(1) {
            let shared = Arc::clone(&shared);
            let tx = tx.clone();
            let notify = Arc::clone(&notify);
            let spawned = thread::Builder::new()
                .name(format!("decode-{i}"))
                .spawn(move || worker(&shared, &tx, &*notify));
            if let Err(e) = spawned {
                tracing::error!("cannot start decoder thread: {e}");
            }
        }
        Self { shared, results }
    }

    /// Replaces all pending requests with `wanted` (highest priority first).
    /// Requests that are already being decoded are skipped.
    pub fn request(&self, wanted: Vec<Request>) {
        let Ok(mut st) = self.shared.state.lock() else {
            return;
        };
        st.queue.clear();
        for r in wanted {
            let k = (cache::key(&r.path), r.quality);
            if !st.running.contains(&k) {
                st.queue.push_back(r);
            }
        }
        drop(st);
        self.shared.wake.notify_all();
    }

    pub fn try_recv(&self) -> Option<Done> {
        self.results.try_recv().ok()
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        if let Ok(mut st) = self.shared.state.lock() {
            st.shutdown = true;
            st.queue.clear();
        }
        self.shared.wake.notify_all();
        // Workers are not joined: a decode in progress must not delay exit.
    }
}

fn worker(shared: &Shared, tx: &Sender<Done>, notify: &(dyn Fn() + Send + Sync)) {
    loop {
        let (req, hint) = {
            let Ok(mut st) = shared.state.lock() else {
                return;
            };
            loop {
                if st.shutdown {
                    return;
                }
                if let Some(r) = st.queue.pop_front() {
                    st.running.push((cache::key(&r.path), r.quality));
                    break (r, st.hint);
                }
                st = match shared.wake.wait(st) {
                    Ok(s) => s,
                    Err(_) => return,
                };
            }
        };

        let hint = match req.quality {
            Quality::Preview => Some(hint),
            Quality::Full => None,
        };
        let result = decode::decode_file(&req.path, hint)
            .map(Arc::new)
            .map_err(|e| e.to_string());

        if let Ok(mut st) = shared.state.lock() {
            let k = (cache::key(&req.path), req.quality);
            st.running.retain(|r| *r != k);
        }
        if tx
            .send(Done {
                path: req.path,
                result,
            })
            .is_err()
        {
            return;
        }
        notify();
    }
}

/// Worker count: decoding is single-threaded per image, so a few images
/// in parallel; leave cores for the UI and the GPU driver.
pub fn default_threads() -> usize {
    thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(2, 6))
}
