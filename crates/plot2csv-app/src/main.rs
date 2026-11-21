#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1400.0, 900.0]).with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("plot2csv", options, Box::new(|cc| Ok(Box::new(plot2csv_app::Plot2CsvApp::new(cc)))))
}

#[cfg(target_arch = "wasm32")]
fn main() {}
