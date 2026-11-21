//! Web Worker entry point. Each worker owns one wasm instance; it receives a
//! JSON `Request`, runs it, and posts back a JSON `Reply`.

#[cfg(target_arch = "wasm32")]
fn main() {
    use evolver_app::tasks::{Reply, Request, handle};
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;
    use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

    console_error_panic_hook::set_once();
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let reply_scope = scope.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let text = event.data().as_string().unwrap_or_default();
        let reply = match serde_json::from_str::<Request>(&text) {
            Ok(req) => handle(req),
            Err(e) => Reply::Error(format!("bad request: {e}")),
        };
        let json = serde_json::to_string(&reply).unwrap_or_else(|e| format!("{{\"Error\":\"{e}\"}}"));
        let _ = reply_scope.post_message(&JsValue::from_str(&json));
    });
    scope.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("This binary is the Web Worker entry point; it does nothing natively.");
}
