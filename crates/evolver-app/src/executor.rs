//! Runs `Request`s in parallel: OS threads natively, a pool of Web Workers in
//! the browser. The UI submits requests and polls for replies every frame.

use evolver_app::tasks::{Reply, Request};

/// Number of hardware threads available to us.
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

#[cfg(not(target_arch = "wasm32"))]
pub use native::Executor;
#[cfg(target_arch = "wasm32")]
pub use web::Executor;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::sync::mpsc::{Receiver, Sender, channel};

    pub struct Executor {
        tx: Sender<Reply>,
        rx: Receiver<Reply>,
        ctx: eframe::egui::Context,
    }

    impl Executor {
        pub fn new(ctx: eframe::egui::Context, _n_workers: usize) -> Self {
            let (tx, rx) = channel();
            Self { tx, rx, ctx }
        }

        /// Run on a new OS thread (the caller submits at most one request per
        /// core, so this behaves as a pool of the right size).
        pub fn submit(&mut self, req: Request) {
            let tx = self.tx.clone();
            let ctx = self.ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(evolver_app::tasks::handle(req));
                ctx.request_repaint();
            });
        }

        pub fn poll(&mut self) -> Vec<Reply> {
            self.rx.try_iter().collect()
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;
    use web_sys::{MessageEvent, Worker};

    struct Shared {
        replies: Vec<Reply>,
        idle: Vec<usize>,
    }

    pub struct Executor {
        workers: Vec<Worker>,
        _handlers: Vec<Closure<dyn FnMut(MessageEvent)>>,
        shared: Rc<RefCell<Shared>>,
        queue: VecDeque<Request>,
    }

    impl Executor {
        pub fn new(ctx: eframe::egui::Context, n_workers: usize) -> Self {
            let shared = Rc::new(RefCell::new(Shared { replies: Vec::new(), idle: Vec::new() }));
            let mut workers = Vec::new();
            let mut handlers = Vec::new();
            for _ in 0..n_workers.max(1) {
                // Trunk builds the `worker` bin with a loader shim (see index.html).
                let worker = match Worker::new("./worker_loader.js") {
                    Ok(w) => w,
                    Err(e) => {
                        web_sys::console::error_1(&e);
                        continue;
                    }
                };
                let index = workers.len();
                let shared2 = shared.clone();
                let ctx2 = ctx.clone();
                let handler = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                    let text = event.data().as_string().unwrap_or_default();
                    let reply = serde_json::from_str::<Reply>(&text)
                        .unwrap_or_else(|e| Reply::Error(format!("bad reply: {e}")));
                    let mut s = shared2.borrow_mut();
                    s.replies.push(reply);
                    s.idle.push(index);
                    ctx2.request_repaint();
                });
                worker.set_onmessage(Some(handler.as_ref().unchecked_ref()));
                shared.borrow_mut().idle.push(index);
                workers.push(worker);
                handlers.push(handler);
            }
            Self { workers, _handlers: handlers, shared, queue: VecDeque::new() }
        }

        pub fn submit(&mut self, req: Request) {
            self.queue.push_back(req);
            self.dispatch();
        }

        fn dispatch(&mut self) {
            while !self.queue.is_empty() {
                let Some(i) = self.shared.borrow_mut().idle.pop() else { return };
                let Some(req) = self.queue.pop_front() else { return };
                let json = serde_json::to_string(&req).unwrap_or_default();
                let _ = self.workers[i].post_message(&JsValue::from_str(&json));
            }
        }

        pub fn poll(&mut self) -> Vec<Reply> {
            self.dispatch();
            std::mem::take(&mut self.shared.borrow_mut().replies)
        }
    }
}
