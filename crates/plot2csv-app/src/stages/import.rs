use crate::app::Plot2CsvApp;
use crate::widgets;

impl Plot2CsvApp {
    pub fn import_sidebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        widgets::section(ui, "① Import image");
        ui.horizontal_wrapped(|ui| {
            if ui.button("📂 Load Image…").clicked() {
                self.open_image_dialog(ctx);
            }
            if ui.button("Import Project…").clicked() {
                self.open_project_dialog(ctx);
            }
        });
        ui.label(egui::RichText::new("You can also drag-and-drop an image, or paste one with Ctrl+V.").weak());
        let Some(src) = &self.project.source else { return };
        ui.add_space(6.0);
        egui::Grid::new("imginfo").num_columns(2).show(ui, |ui| {
            ui.label("File");
            ui.label(&src.name);
            ui.end_row();
            ui.label("Size");
            ui.label(format!("{} × {} px", src.width, src.height));
            ui.end_row();
            if let Some(img) = &self.imgs.source {
                if img.width() != src.width || img.height() != src.height {
                    ui.label("Working size");
                    ui.label(format!("{} × {} px", img.width(), img.height()));
                    ui.end_row();
                }
            }
        });
        ui.add_space(6.0);
        widgets::section(ui, "Options");
        let mut opts = self.project.import.value.clone().unwrap_or_default();
        if ui
            .checkbox(&mut opts.enhance, "Enhance photo (flatten lighting, white balance)")
            .on_hover_text("Useful for phone photos of printed plots. Not needed for screenshots or exported figures.")
            .changed()
        {
            self.project.import.value = Some(opts);
            self.edited(plot2csv_core::Stage::Import);
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "Next, plot2csv will look for rotation and perspective distortion and propose a correction for you to review.",
            )
            .weak(),
        );
    }
}
