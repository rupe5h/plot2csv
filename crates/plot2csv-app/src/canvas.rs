//! Zoomable, pannable image canvas with image<->screen transforms.

use egui::{Color32, Pos2, Rect, Response, Sense, Stroke, TextureHandle, Vec2};

#[derive(Clone, Debug)]
pub struct ImageCanvas {
    /// Screen pixels per image pixel.
    pub zoom: f32,
    /// Screen position of image pixel (0,0), relative to the canvas rect min.
    pub offset: Vec2,
    pub fit_pending: bool,
    pub last_rect: Rect,
    pub image_size: Vec2,
}

impl Default for ImageCanvas {
    fn default() -> Self {
        Self { zoom: 1.0, offset: Vec2::ZERO, fit_pending: true, last_rect: Rect::NOTHING, image_size: Vec2::ZERO }
    }
}

/// What the canvas hands back to the stage code each frame.
pub struct CanvasView {
    pub response: Response,
    pub rect: Rect,
    pub zoom: f32,
    origin: Pos2,
    /// Image coordinates under the pointer, if hovering.
    pub hover: Option<Pos2>,
}

impl CanvasView {
    pub fn to_screen(&self, p: Pos2) -> Pos2 {
        self.origin + p.to_vec2() * self.zoom
    }
    pub fn to_image(&self, s: Pos2) -> Pos2 {
        ((s - self.origin) / self.zoom).to_pos2()
    }
    pub fn rect_to_screen(&self, r: Rect) -> Rect {
        Rect::from_min_max(self.to_screen(r.min), self.to_screen(r.max))
    }
}

impl ImageCanvas {
    pub fn fit(&mut self) {
        self.fit_pending = true;
    }

    pub fn zoom_by(&mut self, factor: f32, anchor: Option<Pos2>) {
        let rect = self.last_rect;
        let anchor = anchor.unwrap_or(rect.center());
        let old = self.zoom;
        self.zoom = (self.zoom * factor).clamp(0.02, 64.0);
        let a = anchor - rect.min;
        self.offset = a - (a - self.offset) * (self.zoom / old);
    }

    pub fn set_zoom(&mut self, z: f32) {
        self.zoom_by(z / self.zoom, None);
    }

    /// Paint the image and handle navigation. `primary_drag_pans` lets the left
    /// button pan when no tool wants it.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tex: Option<&TextureHandle>,
        image_size: Vec2,
        primary_drag_pans: bool,
    ) -> CanvasView {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = response.rect;
        self.last_rect = rect;
        if self.image_size != image_size {
            self.image_size = image_size;
            self.fit_pending = true;
        }
        if self.fit_pending && image_size.x > 0.0 && rect.width() > 10.0 {
            let z = ((rect.width() - 20.0) / image_size.x).min((rect.height() - 20.0) / image_size.y);
            self.zoom = z.clamp(0.02, 64.0);
            self.offset = (rect.size() - image_size * self.zoom) / 2.0;
            self.fit_pending = false;
        }

        // Navigation: wheel zooms about the pointer, middle/secondary drag or space+drag pans.
        if response.hovered() {
            let (scroll, zoom_delta, space) =
                ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.key_down(egui::Key::Space)));
            let pointer = response.hover_pos();
            if zoom_delta != 1.0 {
                self.zoom_by(zoom_delta, pointer);
            } else if scroll.y != 0.0 {
                self.zoom_by((scroll.y / 300.0).exp(), pointer);
            }
            if space && response.dragged_by(egui::PointerButton::Primary) {
                self.offset += response.drag_delta();
            }
        }
        if response.dragged_by(egui::PointerButton::Middle)
            || response.dragged_by(egui::PointerButton::Secondary)
            || (primary_drag_pans && response.dragged_by(egui::PointerButton::Primary))
        {
            self.offset += response.drag_delta();
        }

        let origin = rect.min + self.offset;
        painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
        if let Some(tex) = tex {
            let img_rect = Rect::from_min_size(origin, image_size * self.zoom);
            painter.image(tex.id(), img_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            painter.rect_stroke(img_rect, 0.0, Stroke::new(1.0, Color32::from_gray(90)), egui::StrokeKind::Outside);
        }
        let hover = response.hover_pos().map(|p| ((p - origin) / self.zoom).to_pos2());
        CanvasView { response, rect, zoom: self.zoom, origin, hover }
    }
}
