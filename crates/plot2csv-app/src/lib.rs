pub mod app;
pub mod canvas;
pub mod dialogs;
pub mod images;
pub mod measure;
pub mod platform;
pub mod stages;
pub mod widgets;

pub use app::Plot2CsvApp;

#[cfg(target_arch = "wasm32")]
mod web_entry {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    /// Called by the page's boot script once the wasm module is instantiated.
    #[wasm_bindgen]
    pub async fn start(canvas_id: String) -> Result<(), JsValue> {
        console_error_panic_hook::set_once();
        eframe::WebLogger::init(log::LevelFilter::Info).ok();
        let doc = web_sys::window().unwrap().document().unwrap();
        let canvas: web_sys::HtmlCanvasElement = doc
            .get_element_by_id(&canvas_id)
            .ok_or_else(|| JsValue::from_str("canvas not found"))?
            .dyn_into()?;
        let runner = eframe::WebRunner::new();
        runner
            .start(canvas, eframe::WebOptions::default(), Box::new(|cc| Ok(Box::new(crate::Plot2CsvApp::new(cc)))))
            .await?;
        // Keep the runner alive for the page lifetime.
        std::mem::forget(runner);
        Ok(())
    }

    /// Test hook: run the full OCR pipeline on encoded image bytes, return JSON lines.
    #[wasm_bindgen]
    pub async fn ocr_image_json(bytes: Vec<u8>) -> Result<String, JsValue> {
        let img = image::load_from_memory(&bytes).map_err(|e| JsValue::from_str(&e.to_string()))?;
        let gray = plot2csv_vision::raster::rgba_to_gray(&img.to_rgba8());
        let rx = crate::platform::run_ocr(&egui::Context::default(), gray, None);
        // The web job completes asynchronously; poll the channel between yields.
        loop {
            match rx.try_recv() {
                Ok(r) => {
                    let lines = r.map_err(|e| JsValue::from_str(&e))?;
                    return Ok(serde_json::to_string(&lines).unwrap());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    let p = js_sys::Promise::new(&mut |res, _| {
                        web_sys::window().unwrap().set_timeout_with_callback_and_timeout_and_arguments_0(&res, 10).unwrap();
                    });
                    wasm_bindgen_futures::JsFuture::from(p).await?;
                }
                Err(e) => return Err(JsValue::from_str(&e.to_string())),
            }
        }
    }
}
