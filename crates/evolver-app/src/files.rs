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
}

/// An opened file: what it was opened for, its name, and its text (or an error).
pub type Opened = (OpenKind, String, Result<String, String>);

#[derive(Default)]
pub struct Files {
    opened: Rc<RefCell<Vec<Opened>>>,
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
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(file) = dialog.pick_file().await {
                    let bytes = file.read().await;
                    opened.borrow_mut().push((kind, file.file_name(), Ok(String::from_utf8_lossy(&bytes).into_owned())));
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

    /// Save several text files: into a chosen folder on the desktop, as one
    /// combined download in the browser.
    pub fn save_many(&self, combined_name: &str, files: &[(String, String)]) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = combined_name;
            let dir = rfd::FileDialog::new().set_title("Choose a folder for the models").pick_folder()?;
            let errors = files.iter().filter(|(name, content)| std::fs::write(dir.join(name), content).is_err()).count();
            Some(if errors == 0 {
                format!("Saved {} models to {}", files.len(), dir.display())
            } else {
                format!("Saved {} of {} models to {}", files.len() - errors, files.len(), dir.display())
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let mut all = String::new();
            for (name, content) in files {
                all.push_str(&format!("// ===== {name} =====\n{content}\n\n"));
            }
            self.save_text(combined_name, &all)
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn download(name: &str, content: &str) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let parts = js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(content));
    let props = web_sys::BlobPropertyBag::new();
    props.set_type("text/plain");
    let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &props).map_err(err)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(err)?;
    let a: web_sys::HtmlAnchorElement =
        document.create_element("a").map_err(err)?.dyn_into().map_err(|_| "not an anchor".to_string())?;
    a.set_href(&url);
    a.set_download(name);
    a.click();
    web_sys::Url::revoke_object_url(&url).map_err(err)?;
    Ok(())
}
