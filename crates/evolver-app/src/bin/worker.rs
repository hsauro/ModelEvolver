//! Web Worker entry point. Each worker owns one wasm instance and runs one
//! experiment per `RunRequest`, posting JSON `Reply`s (progress, then the
//! result) back to the page. Cancelling = the page terminates the worker.

#[cfg(target_arch = "wasm32")]
fn main() {
    use evolver_app::tasks::{Reply, RunRequest, run_experiment};
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;
    use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

    console_error_panic_hook::set_once();
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let reply_scope = scope.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let text = event.data().as_string().unwrap_or_default();
        let Ok(req) = serde_json::from_str::<RunRequest>(&text) else {
            web_sys::console::error_1(&JsValue::from_str("worker: bad request"));
            return;
        };
        run_experiment(
            req,
            |reply| {
                if let Ok(json) = serde_json::to_string(&reply) {
                    let _ = reply_scope.post_message(&JsValue::from_str(&json));
                }
            },
            || false,
        );
    });
    scope.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();
    // Tell the page we are listening; it holds requests until then.
    if let Ok(json) = serde_json::to_string(&Reply::Ready) {
        let _ = scope.post_message(&JsValue::from_str(&json));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("This binary is the Web Worker entry point; it does nothing natively.");
}
