//! Platform services: file I/O, clipboard paste, background jobs and the OCR
//! backend. The web and native implementations share this interface.

use std::sync::mpsc::{channel, Receiver};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;
#[cfg(target_arch = "wasm32")]
pub use web::*;

/// A file picked by the user.
pub struct PickedFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Ask the user for a file; the result arrives on the returned channel.
pub fn pick_file(ctx: &egui::Context, filters: &[(&str, &[&str])]) -> Receiver<PickedFile> {
    let (tx, rx) = channel();
    let mut dlg = rfd::AsyncFileDialog::new();
    for (name, ext) in filters {
        dlg = dlg.add_filter(*name, ext);
    }
    let ctx = ctx.clone();
    spawn_local(async move {
        if let Some(h) = dlg.pick_file().await {
            let bytes = h.read().await;
            let _ = tx.send(PickedFile { name: h.file_name(), bytes });
            ctx.request_repaint();
        }
    });
    rx
}

/// Run a (possibly heavy) closure off the UI path when the platform allows,
/// delivering its result through a channel and requesting a repaint.
pub fn run_job<T: Send + 'static>(ctx: &egui::Context, f: impl FnOnce() -> T + Send + 'static) -> Receiver<T> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    spawn_blocking(move || {
        let r = f();
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    rx
}

/// Read a file dropped onto the window; the bytes arrive on the returned channel.
pub fn read_dropped(ctx: &egui::Context, f: egui::DroppedFileHandle) -> Receiver<PickedFile> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    let name = f.path().file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    #[cfg(target_arch = "wasm32")]
    spawn_local(async move {
        match f.bytes_async().await {
            Ok(bytes) => {
                let _ = tx.send(PickedFile { name, bytes });
            }
            Err(e) => log::error!("reading dropped file: {e}"),
        }
        ctx.request_repaint();
    });
    #[cfg(not(target_arch = "wasm32"))]
    match f.bytes() {
        Ok(bytes) => {
            let _ = tx.send(PickedFile { name, bytes });
            ctx.request_repaint();
        }
        Err(e) => log::error!("reading dropped file: {e}"),
    }
    rx
}
