//! Runs a batch (or a continuous series) of experiments on a pool of workers:
//! OS threads natively, Web Workers in the browser. The UI calls `poll` every
//! frame; the runner dispatches new experiments to idle workers and returns
//! finished results.

use std::collections::BTreeMap;

use evolver_app::tasks::{Reply, RunRequest};
use evolver_core::config::EvolverConfig;
use evolver_core::ga::{ExperimentEvent, ExperimentResult, Progress};
use evolver_core::network::Network;
use evolver_core::objective::Target;
use web_time::Instant;

/// Number of hardware threads on this machine (browser: navigator.hardwareConcurrency).
pub fn hardware_threads() -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    }
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window().map(|w| w.navigator().hardware_concurrency() as usize).unwrap_or(1).max(1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Idle,
    Running,
    /// No new experiments are started; running ones finish.
    Stopping,
}

/// A running experiment as seen from the UI.
pub struct LiveExperiment {
    pub id: u32,
    pub started: Instant,
    pub progress: Option<Progress>,
    pub best_network: Option<Network>,
    /// (generation, best fitness), for the live plot.
    pub history: Vec<[f64; 2]>,
}

pub enum RunnerEvent {
    /// The result and how long the experiment took (seconds).
    Finished(Box<ExperimentResult>, f64),
    Failed { id: u32, message: String },
}

pub struct Runner {
    pool: pool::Pool,
    slot_exp: Vec<Option<u32>>,
    pub live: BTreeMap<u32, LiveExperiment>,
    pub state: RunState,
    template: Option<(EvolverConfig, Target, u32)>,
    next_id: u32,
    /// None = continuous.
    total: Option<u32>,
    /// Desired number of concurrent experiments (CPU usage setting).
    pub target_workers: usize,
    pub started_at: Option<Instant>,
    pub ended_at: Option<Instant>,
    /// When the user pressed Stop.
    pub stopped_at: Option<Instant>,
    pub base_seed: u32,
}

impl Runner {
    pub fn new(ctx: eframe::egui::Context) -> Self {
        Self {
            pool: pool::Pool::new(ctx),
            slot_exp: Vec::new(),
            live: BTreeMap::new(),
            state: RunState::Idle,
            template: None,
            next_id: 0,
            total: None,
            target_workers: 1,
            started_at: None,
            ended_at: None,
            stopped_at: None,
            base_seed: 0,
        }
    }

    pub fn start(&mut self, cfg: EvolverConfig, target: Target, base_seed: u32, workers: usize) {
        self.total = (!cfg.continuous).then_some(cfg.num_experiments.max(1));
        self.template = Some((cfg, target, base_seed));
        self.base_seed = base_seed;
        self.next_id = 0;
        self.target_workers = workers.max(1);
        self.state = RunState::Running;
        self.started_at = Some(Instant::now());
        self.ended_at = None;
        self.stopped_at = None;
        self.live.clear();
        self.dispatch();
    }

    /// Finish the running experiments but start no new ones.
    pub fn stop(&mut self) {
        if self.state == RunState::Running {
            self.state = RunState::Stopping;
            self.stopped_at = Some(Instant::now());
            self.check_done();
        }
    }

    /// Abandon everything immediately.
    pub fn cancel(&mut self) {
        self.pool.cancel_all();
        self.slot_exp.iter_mut().for_each(|s| *s = None);
        self.live.clear();
        self.state = RunState::Idle;
        self.ended_at = Some(Instant::now());
    }

    pub fn elapsed_secs(&self) -> f64 {
        match (self.started_at, self.ended_at) {
            (Some(s), Some(e)) => (e - s).as_secs_f64(),
            (Some(s), None) => s.elapsed().as_secs_f64(),
            _ => 0.0,
        }
    }

    /// The clock for the throughput figures: the whole run, except that it
    /// stops when the user presses Stop (the experiments still finishing after
    /// an explicit Stop do not drag the figures down).
    pub fn active_secs(&self) -> f64 {
        match (self.started_at, self.stopped_at.or(self.ended_at)) {
            (Some(s), Some(e)) => (e - s).as_secs_f64(),
            (Some(s), None) => s.elapsed().as_secs_f64(),
            _ => 0.0,
        }
    }

    /// Experiments started so far, and the batch size (None = continuous).
    pub fn counts(&self) -> (u32, Option<u32>) {
        (self.next_id, self.total)
    }

    fn running(&self) -> usize {
        self.slot_exp.iter().filter(|s| s.is_some()).count()
    }

    fn dispatch(&mut self) {
        if self.state != RunState::Running {
            return;
        }
        let Some((cfg, target, seed)) = self.template.clone() else { return };
        while self.running() < self.target_workers {
            if self.total.is_some_and(|t| self.next_id >= t) {
                // A batch that has started all its experiments keeps its clock
                // running until they finish: that tail is part of the batch's cost.
                self.state = RunState::Stopping;
                break;
            }
            let slot = match self.slot_exp.iter().position(|s| s.is_none()) {
                Some(s) => s,
                None => {
                    self.slot_exp.push(None);
                    self.slot_exp.len() - 1
                }
            };
            let id = self.next_id;
            self.next_id += 1;
            self.slot_exp[slot] = Some(id);
            self.live.insert(
                id,
                LiveExperiment { id, started: Instant::now(), progress: None, best_network: None, history: Vec::new() },
            );
            self.pool.start(slot, RunRequest { id, base_seed: seed, cfg: cfg.clone(), target: target.clone() });
        }
        self.check_done();
    }

    fn check_done(&mut self) {
        if self.state == RunState::Stopping && self.running() == 0 {
            self.state = RunState::Idle;
            self.ended_at = Some(Instant::now());
        }
    }

    pub fn poll(&mut self) -> Vec<RunnerEvent> {
        let mut out = Vec::new();
        for (slot, reply) in self.pool.poll() {
            let event = match reply {
                Reply::Event(e) => e,
                Reply::Ready => continue,
                Reply::WorkerError(message) => {
                    if let Some(Some(id)) = self.slot_exp.get(slot).copied() {
                        self.free(slot, id);
                        self.pool.discard(slot);
                        out.push(RunnerEvent::Failed { id, message: format!("worker error: {message}") });
                    }
                    continue;
                }
            };
            match event {
                ExperimentEvent::Progress(p) => {
                    if let Some(l) = self.live.get_mut(&p.id) {
                        l.history.push([f64::from(p.generation), p.best_fitness]);
                        if let Some(net) = &p.best_network {
                            l.best_network = Some(net.clone());
                        }
                        l.progress = Some(p);
                    }
                }
                ExperimentEvent::Finished(r) => {
                    let secs = self.live.get(&r.id).map_or(0.0, |l| l.started.elapsed().as_secs_f64());
                    self.free(slot, r.id);
                    out.push(RunnerEvent::Finished(r, secs));
                }
                ExperimentEvent::Failed { id, message } => {
                    self.free(slot, id);
                    out.push(RunnerEvent::Failed { id, message });
                }
            }
        }
        self.dispatch();
        self.check_done();
        out
    }

    fn free(&mut self, slot: usize, id: u32) {
        if let Some(s) = self.slot_exp.get_mut(slot)
            && *s == Some(id)
        {
            *s = None;
        }
        self.live.remove(&id);
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod pool {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};

    /// One OS thread per running experiment (at most `target_workers` at once).
    pub struct Pool {
        tx: Sender<(u64, usize, Reply)>,
        rx: Receiver<(u64, usize, Reply)>,
        ctx: eframe::egui::Context,
        epoch: u64,
        cancel: Arc<AtomicBool>,
    }

    impl Pool {
        pub fn new(ctx: eframe::egui::Context) -> Self {
            let (tx, rx) = channel();
            Self { tx, rx, ctx, epoch: 0, cancel: Arc::new(AtomicBool::new(false)) }
        }

        pub fn start(&mut self, slot: usize, req: RunRequest) {
            let (tx, ctx, epoch, cancel) = (self.tx.clone(), self.ctx.clone(), self.epoch, self.cancel.clone());
            std::thread::spawn(move || {
                evolver_app::tasks::run_experiment(
                    req,
                    |reply| {
                        let _ = tx.send((epoch, slot, reply));
                        ctx.request_repaint();
                    },
                    || cancel.load(Ordering::Relaxed),
                );
            });
        }

        /// Nothing to do natively (a thread that failed has already exited).
        pub fn discard(&mut self, _slot: usize) {}

        pub fn cancel_all(&mut self) {
            self.cancel.store(true, Ordering::Relaxed);
            self.cancel = Arc::new(AtomicBool::new(false));
            self.epoch += 1;
        }

        pub fn poll(&mut self) -> Vec<(usize, Reply)> {
            let epoch = self.epoch;
            self.rx.try_iter().filter(|(e, _, _)| *e == epoch).map(|(_, s, r)| (s, r)).collect()
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod pool {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;
    use web_sys::{MessageEvent, Worker};

    struct Slot {
        worker: Worker,
        /// A request waiting for the worker to report Ready.
        pending: Rc<RefCell<Option<Vec<u8>>>>,
        ready: Rc<std::cell::Cell<bool>>,
        _handler: Closure<dyn FnMut(MessageEvent)>,
        _on_error: Closure<dyn FnMut(web_sys::ErrorEvent)>,
    }

    /// One Web Worker per slot, created on demand; cancel terminates them all.
    pub struct Pool {
        slots: Vec<Option<Slot>>,
        replies: Rc<RefCell<Vec<(usize, Reply)>>>,
        ctx: eframe::egui::Context,
    }

    impl Pool {
        pub fn new(ctx: eframe::egui::Context) -> Self {
            Self { slots: Vec::new(), replies: Rc::new(RefCell::new(Vec::new())), ctx }
        }

        fn slot(&mut self, index: usize) -> Option<&Slot> {
            while self.slots.len() <= index {
                self.slots.push(None);
            }
            if self.slots[index].is_none() {
                // Trunk builds the `worker` bin with a loader shim (index.html).
                let worker = Worker::new("./worker_loader.js").ok()?;
                let pending: Rc<RefCell<Option<Vec<u8>>>> = Rc::new(RefCell::new(None));
                let ready = Rc::new(std::cell::Cell::new(false));
                let (replies, ctx, w2, p2, r2) =
                    (self.replies.clone(), self.ctx.clone(), worker.clone(), pending.clone(), ready.clone());
                let handler = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                    let bytes = js_sys::Uint8Array::new(&event.data()).to_vec();
                    match evolver_app::tasks::decode::<Reply>(&bytes) {
                        Ok(Reply::Ready) => {
                            r2.set(true);
                            if let Some(req) = p2.borrow_mut().take() {
                                let _ = w2.post_message(&js_sys::Uint8Array::from(&req[..]));
                            }
                        }
                        Ok(reply) => {
                            replies.borrow_mut().push((index, reply));
                            ctx.request_repaint();
                        }
                        Err(e) => {
                            replies.borrow_mut().push((index, Reply::WorkerError(format!("unreadable message: {e}"))));
                            ctx.request_repaint();
                        }
                    }
                });
                worker.set_onmessage(Some(handler.as_ref().unchecked_ref()));
                let (replies, ctx) = (self.replies.clone(), self.ctx.clone());
                let on_error = Closure::<dyn FnMut(web_sys::ErrorEvent)>::new(move |e: web_sys::ErrorEvent| {
                    replies.borrow_mut().push((index, Reply::WorkerError(e.message())));
                    ctx.request_repaint();
                });
                worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
                self.slots[index] = Some(Slot { worker, pending, ready, _handler: handler, _on_error: on_error });
            }
            self.slots[index].as_ref()
        }

        pub fn start(&mut self, index: usize, req: RunRequest) {
            let bytes = evolver_app::tasks::encode(&req);
            if let Some(slot) = self.slot(index) {
                if slot.ready.get() {
                    let _ = slot.worker.post_message(&js_sys::Uint8Array::from(&bytes[..]));
                } else {
                    *slot.pending.borrow_mut() = Some(bytes);
                }
            }
        }

        /// Terminate a broken worker; a fresh one is created on next use.
        pub fn discard(&mut self, index: usize) {
            if let Some(slot) = self.slots.get_mut(index).and_then(Option::take) {
                slot.worker.terminate();
            }
        }

        pub fn cancel_all(&mut self) {
            for s in self.slots.iter_mut() {
                if let Some(slot) = s.take() {
                    slot.worker.terminate();
                }
            }
            self.replies.borrow_mut().clear();
        }

        pub fn poll(&mut self) -> Vec<(usize, Reply)> {
            std::mem::take(&mut *self.replies.borrow_mut())
        }
    }
}
