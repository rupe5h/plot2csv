use std::future::Future;
use std::sync::mpsc::{channel, Receiver};

use plot2csv_ocr::TextLine;

pub fn spawn_local<F: Future<Output = ()> + 'static>(f: F) {
    // Native futures here (file dialogs, ORT) are all fine to block on.
    pollster::block_on(f);
}

pub fn spawn_blocking(f: impl FnOnce() + Send + 'static) {
    std::thread::spawn(f);
}

/// Save bytes to a file chosen by the user.
pub fn save_file(name: &str, bytes: Vec<u8>) {
    if let Some(path) = rfd::FileDialog::new().set_file_name(name).save_file() {
        if let Err(e) = std::fs::write(&path, bytes) {
            log::error!("saving {}: {e}", path.display());
        }
    }
}

/// Images pasted from the clipboard are delivered through egui on native.
pub fn take_pasted_image() -> Option<Vec<u8>> {
    None
}

pub fn ocr_available() -> bool {
    cfg!(feature = "native-ocr")
}

#[cfg(feature = "native-ocr")]
mod ocr {
    use super::*;
    use std::sync::{Arc, OnceLock};
    use plot2csv_ocr::backend::native::NativeOrt;
    use plot2csv_ocr::IndraConfig;

    static ENGINE: OnceLock<Result<(Arc<NativeOrt>, IndraConfig), String>> = OnceLock::new();

    fn models_dir() -> std::path::PathBuf {
        std::env::var_os("PLOT2CSV_MODELS")
            .map(Into::into)
            .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../vendor/models").into())
    }

    fn engine() -> Result<(Arc<NativeOrt>, IndraConfig), String> {
        ENGINE
            .get_or_init(|| {
                let d = models_dir();
                let load = || -> anyhow::Result<_> {
                    let cfg = IndraConfig::from_meta_json(&std::fs::read_to_string(d.join("meta.json"))?)?;
                    let be = NativeOrt::new(&std::fs::read(d.join("det.onnx"))?, &std::fs::read(d.join("rec.onnx"))?)?;
                    Ok((Arc::new(be), cfg))
                };
                load().map_err(|e| format!("loading OCR models from {}: {e}", d.display()))
            })
            .clone()
    }

    pub fn run_ocr(
        ctx: &egui::Context,
        gray: image::GrayImage,
        boxes: Option<Vec<[f32; 4]>>,
    ) -> Receiver<Result<Vec<TextLine>, String>> {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = engine().and_then(|(be, cfg)| {
                pollster::block_on(async {
                    match boxes {
                        Some(b) => plot2csv_ocr::indra::recognize(&*be, &cfg, &gray, &b).await,
                        None => plot2csv_ocr::indra::ocr_page(&*be, &cfg, &gray).await,
                    }
                })
                .map_err(|e| e.to_string())
            });
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        rx
    }
}

#[cfg(feature = "native-ocr")]
pub use ocr::run_ocr;

#[cfg(not(feature = "native-ocr"))]
pub fn run_ocr(
    _ctx: &egui::Context,
    _gray: image::GrayImage,
    _boxes: Option<Vec<[f32; 4]>>,
) -> Receiver<Result<Vec<TextLine>, String>> {
    let (tx, rx) = channel();
    let _ = tx.send(Err("OCR needs the `native-ocr` feature in native builds".into()));
    rx
}
