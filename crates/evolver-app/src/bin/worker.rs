//! Web Worker entry point. Each worker owns one wasm instance and runs one
//! experiment per `RunRequest`, posting JSON `Reply`s (progress, then the
//! result) back to the page. Cancelling = the page terminates the worker.

#[cfg(target_arch = "wasm32")]
fn main() {
    use evolver_app::tasks::{Reply, RunRequest, decode, encode, run_experiment};
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;
    use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

    console_error_panic_hook::set_once();
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let reply_scope = scope.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let bytes = js_sys::Uint8Array::new(&event.data()).to_vec();
        let Ok(req) = decode::<RunRequest>(&bytes) else {
            web_sys::console::error_1(&JsValue::from_str("worker: bad request"));
            return;
        };
        run_experiment(
            req,
            |reply| {
                let _ = reply_scope.post_message(&js_sys::Uint8Array::from(&encode(&reply)[..]));
            },
            || false,
        );
    });
    scope.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();
    // Tell the page we are listening; it holds requests until then.
    let _ = scope.post_message(&js_sys::Uint8Array::from(&encode(&Reply::Ready)[..]));
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("This binary is the Web Worker entry point; it does nothing natively.");
}
