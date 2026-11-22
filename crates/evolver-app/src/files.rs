//! Opening and saving files: native dialogs on the desktop, file pickers and
//! downloads in the browser. Opening is asynchronous in the browser, so opened
//! files are delivered through `poll`.

use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenKind {
    Target,
    Patterns,
    PascalIni,
    SettingsJson,
    Session,
}

/// An opened file: what it was opened for, its name, and its text (or an error).
pub type Opened = (OpenKind, String, Result<String, String>);

#[derive(Default)]
pub struct Files {
    opened: Rc<RefCell<Vec<Opened>>>,
    /// Used to wake the UI when a file has been read in the background.
    pub ctx: Option<eframe::egui::Context>,
}

impl Files {
    /// Ask the user for a text file; it arrives through `poll`.
    pub fn open(&self, kind: OpenKind, title: &str, ext: &[&str]) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let picked =
                rfd::FileDialog::new().set_title(title).add_filter("files", ext).add_filter("all", &["*"]).pick_file();
            if let Some(path) = picked {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let text = std::fs::read(&path)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .map_err(|e| e.to_string());
                self.opened.borrow_mut().push((kind, name, text));
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let dialog =
                rfd::AsyncFileDialog::new().set_title(title).add_filter("files", ext).add_filter("all", &["*"]);
            let opened = self.opened.clone();
            let ctx = self.ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(file) = dialog.pick_file().await {
                    let bytes = file.read().await;
                    opened.borrow_mut().push((kind, file.file_name(), Ok(String::from_utf8_lossy(&bytes).into_owned())));
                    // Without this the app would only notice the file at the next mouse movement.
                    if let Some(ctx) = ctx {
                        ctx.request_repaint();
                    }
                }
            });
        }
    }

    pub fn poll(&self) -> Vec<Opened> {
        std::mem::take(&mut *self.opened.borrow_mut())
    }

    /// Save one text file. Returns a status message (None if cancelled).
    pub fn save_text(&self, suggested: &str, content: &str) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = rfd::FileDialog::new().set_file_name(suggested).save_file()?;
            Some(match std::fs::write(&path, content) {
                Ok(()) => format!("Saved {}", path.display()),
                Err(e) => format!("Could not save {}: {e}", path.display()),
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            Some(match download(suggested, content) {
                Ok(()) => format!("Downloaded {suggested}"),
                Err(e) => format!("Download failed: {e}"),
            })
        }
    }

    /// Save several text files as one ZIP archive.
    pub fn save_zip(&self, suggested: &str, files: &[(String, String)]) -> Option<String> {
        let bytes = crate::zipfile::zip_store(files);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = rfd::FileDialog::new().set_file_name(suggested).add_filter("zip", &["zip"]).save_file()?;
            Some(match std::fs::write(&path, &bytes) {
                Ok(()) => format!("Saved {} models to {}", files.len(), path.display()),
                Err(e) => format!("Could not save {}: {e}", path.display()),
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            Some(match download_bytes(suggested, &bytes, "application/zip") {
                Ok(()) => format!("Downloaded {suggested} ({} models)", files.len()),
                Err(e) => format!("Download failed: {e}"),
            })
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn download(name: &str, content: &str) -> Result<(), String> {
    download_bytes(name, content.as_bytes(), "text/plain")
}

#[cfg(target_arch = "wasm32")]
fn download_bytes(name: &str, bytes: &[u8], mime: &str) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let props = web_sys::BlobPropertyBag::new();
    props.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &props).map_err(err)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(err)?;
    let a: web_sys::HtmlAnchorElement =
        document.create_element("a").map_err(err)?.dyn_into().map_err(|_| "not an anchor".to_string())?;
    a.set_href(&url);
    a.set_download(name);
    a.click();
    web_sys::Url::revoke_object_url(&url).map_err(err)?;
    Ok(())
}
