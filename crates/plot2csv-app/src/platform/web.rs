use std::future::Future;
use std::sync::mpsc::{channel, Receiver};

use js_sys::{Array, BigInt64Array, Float32Array, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use plot2csv_ocr::backend::{LocalFuture, Model, OcrBackend, Tensor, TensorData};
use plot2csv_ocr::{IndraConfig, TextLine};

pub fn spawn_local<F: Future<Output = ()> + 'static>(f: F) {
    wasm_bindgen_futures::spawn_local(f);
}

/// The browser build is single threaded: run now. Callers show a busy overlay
/// for a frame before starting heavy work.
pub fn spawn_blocking(f: impl FnOnce() + Send + 'static) {
    f();
}

/// Trigger a browser download of `bytes`.
pub fn save_file(name: &str, bytes: Vec<u8>) {
    let r = (|| -> Result<(), JsValue> {
        let arr = Uint8Array::from(bytes.as_slice());
        let parts = Array::new();
        parts.push(&arr.buffer());
        let blob = web_sys::Blob::new_with_buffer_source_sequence(&parts)?;
        let url = web_sys::Url::create_object_url_with_blob(&blob)?;
        let doc = web_sys::window().unwrap().document().unwrap();
        let a: web_sys::HtmlAnchorElement = doc.create_element("a")?.dyn_into()?;
        a.set_href(&url);
        a.set_download(name);
        a.click();
        web_sys::Url::revoke_object_url(&url)?;
        Ok(())
    })();
    if let Err(e) = r {
        log::error!("download failed: {e:?}");
    }
}

/// The page can save a copy of itself (see `wpSaveOffline` in index.template.html).
pub fn offline_copy_available() -> bool {
    Reflect::get(&js_sys::global(), &"wpSaveOffline".into()).is_ok_and(|f| f.is_function())
}

/// Download the self-contained HTML page, as it was loaded, so it can be
/// opened later without a network connection. Returns its size in bytes.
pub fn save_offline_copy(name: &str) -> Result<usize, String> {
    let f: js_sys::Function = Reflect::get(&js_sys::global(), &"wpSaveOffline".into())
        .ok()
        .and_then(|f| f.dyn_into().ok())
        .ok_or("this page cannot save an offline copy")?;
    let size = f.call1(&JsValue::NULL, &name.into()).map_err(|e| format!("{e:?}"))?;
    Ok(size.as_f64().unwrap_or(0.0) as usize)
}

/// Images pasted with Ctrl+V are queued by the page script in `globalThis.wpPasted`.
pub fn take_pasted_image() -> Option<Vec<u8>> {
    let g = js_sys::global();
    let q = Reflect::get(&g, &"wpPasted".into()).ok()?;
    let q: Array = q.dyn_into().ok()?;
    if q.length() == 0 {
        return None;
    }
    let v = q.shift();
    let arr: Uint8Array = v.dyn_into().ok()?;
    Some(arr.to_vec())
}

fn session_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok()?
}

/// Read a value from the tab's `sessionStorage` (None if unavailable, e.g. blocked).
pub fn session_get(key: &str) -> Option<String> {
    session_storage()?.get_item(key).ok()?
}

/// Write a value to the tab's `sessionStorage`; failures are ignored.
pub fn session_set(key: &str, value: &str) {
    if let Some(s) = session_storage() {
        let _ = s.set_item(key, value);
    }
}

fn wp_ort() -> Option<JsValue> {
    let v = Reflect::get(&js_sys::global(), &"wpOrt".into()).ok()?;
    if v.is_undefined() {
        None
    } else {
        Some(v)
    }
}

pub fn ocr_available() -> bool {
    wp_ort().is_some()
}

fn call_method(obj: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
    let f: js_sys::Function = Reflect::get(obj, &name.into())?.dyn_into()?;
    let a = Array::new();
    for x in args {
        a.push(x);
    }
    f.apply(obj, &a)
}

async fn call_async(obj: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
    let p: js_sys::Promise = call_method(obj, name, args)?.dyn_into()?;
    JsFuture::from(p).await
}

/// onnxruntime-web through `globalThis.wpOrt` (see web/ort_bridge.js).
pub struct WebOrt;

impl WebOrt {
    async fn run_js(&self, model: Model, inputs: Vec<(&'static str, Tensor)>, outputs: &[&'static str]) -> anyhow::Result<Vec<Tensor>> {
        let ort = wp_ort().ok_or_else(|| anyhow::anyhow!("onnxruntime bridge not loaded"))?;
        let feeds = Object::new();
        for (name, t) in inputs {
            let o = Object::new();
            let dims = Array::new();
            for d in &t.dims {
                dims.push(&JsValue::from_f64(*d as f64));
            }
            let (ty, data): (&str, JsValue) = match &t.data {
                TensorData::F32(v) => ("float32", Float32Array::from(v.as_slice()).into()),
                TensorData::I64(v) => ("int64", BigInt64Array::from(v.as_slice()).into()),
            };
            Reflect::set(&o, &"type".into(), &ty.into()).ok();
            Reflect::set(&o, &"data".into(), &data).ok();
            Reflect::set(&o, &"dims".into(), &dims).ok();
            Reflect::set(&feeds, &name.into(), &o).ok();
        }
        let outs = Array::new();
        for n in outputs {
            outs.push(&(*n).into());
        }
        let res = call_async(&ort, "run", &[model.name().into(), feeds.into(), outs.into()])
            .await
            .map_err(|e| anyhow::anyhow!("onnxruntime: {e:?}"))?;
        let res: Array = res.dyn_into().map_err(|_| anyhow::anyhow!("bad result"))?;
        let mut v = Vec::new();
        for r in res.iter() {
            let ty = Reflect::get(&r, &"type".into()).ok().and_then(|t| t.as_string()).unwrap_or_default();
            let dims: Array = Reflect::get(&r, &"dims".into()).unwrap().dyn_into().unwrap();
            let dims: Vec<usize> = dims.iter().map(|d| d.as_f64().unwrap_or(0.0) as usize).collect();
            let data = Reflect::get(&r, &"data".into()).unwrap();
            let t = match ty.as_str() {
                "float32" => Tensor::f32(dims, data.dyn_into::<Float32Array>().unwrap().to_vec()),
                "int64" => Tensor::i64(dims, data.dyn_into::<BigInt64Array>().unwrap().to_vec()),
                other => anyhow::bail!("unsupported tensor type {other}"),
            };
            v.push(t);
        }
        Ok(v)
    }
}

impl OcrBackend for WebOrt {
    fn run<'a>(
        &'a self,
        model: Model,
        inputs: Vec<(&'static str, Tensor)>,
        outputs: &'a [&'static str],
    ) -> LocalFuture<'a, anyhow::Result<Vec<Tensor>>> {
        Box::pin(self.run_js(model, inputs, outputs))
    }
}

async fn indra_config() -> anyhow::Result<IndraConfig> {
    thread_local! {
        static CFG: std::cell::RefCell<Option<IndraConfig>> = const { std::cell::RefCell::new(None) };
    }
    if let Some(c) = CFG.with(|c| c.borrow().clone()) {
        return Ok(c);
    }
    let ort = wp_ort().ok_or_else(|| anyhow::anyhow!("onnxruntime bridge not loaded"))?;
    let text = call_async(&ort, "metaText", &[])
        .await
        .map_err(|e| anyhow::anyhow!("loading OCR meta: {e:?}"))?
        .as_string()
        .unwrap_or_default();
    let cfg = IndraConfig::from_meta_json(&text)?;
    CFG.with(|c| *c.borrow_mut() = Some(cfg.clone()));
    Ok(cfg)
}

pub fn run_ocr(
    ctx: &egui::Context,
    gray: image::GrayImage,
    boxes: Option<Vec<[f32; 4]>>,
) -> Receiver<Result<Vec<TextLine>, String>> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    spawn_local(async move {
        let r = async {
            let cfg = indra_config().await?;
            let be = WebOrt;
            match boxes {
                Some(b) => plot2csv_ocr::indra::recognize(&be, &cfg, &gray, &b).await,
                None => plot2csv_ocr::indra::ocr_page(&be, &cfg, &gray).await,
            }
        }
        .await
        .map_err(|e| e.to_string());
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    rx
}
