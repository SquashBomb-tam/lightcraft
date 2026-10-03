//! Window regions and panels.

pub mod bottombar;
pub mod compare;
pub mod crop_overlay;
pub mod detail;
pub mod dialogs;
pub mod edit;
pub mod filterbar;
pub mod grid;
pub mod left;
pub mod mask_handles;
pub mod masking;
pub mod presets;
pub mod profiles;
pub mod right;
pub mod settings;
pub mod strip;
pub mod topbar;

use egui::{Align2, Rect, pos2, vec2};

use crate::LightcraftApp;
use crate::theme::Tokens;

/// The HUD toast at the bottom centre of the canvas.
pub fn toast(app: &mut LightcraftApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    let Some((text, until)) = app.ui.toast.clone() else { return };
    if now > until {
        app.ui.toast = None;
        return;
    }
    let Some(canvas) = app.canvas_rect else { return };
    let t = Tokens::get(ctx);
    let fade = ((until - now) / 0.25).clamp(0.0, 1.0) as f32;
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("toast")));
    let galley = painter.layout_no_wrap(text, t.font(14.0), t.text.gamma_multiply(fade));
    let size = vec2(galley.size().x + 40.0, 40.0);
    let r = Rect::from_center_size(pos2(canvas.center().x, canvas.bottom() - 60.0), size);
    painter.rect_filled(r, 6.0, egui::Color32::from_black_alpha((200.0 * fade) as u8));
    painter.galley(r.center() - galley.size() / 2.0, galley, t.text);
    ctx.request_repaint_after(std::time::Duration::from_millis(30));
}

/// Paint a centred, dimmed message (empty states).
pub fn empty_message(ui: &egui::Ui, rect: Rect, title: &str, body: &str) {
    let t = Tokens::get(ui.ctx());
    let p = ui.painter();
    p.text(rect.center() - vec2(0.0, 12.0), Align2::CENTER_CENTER, title, t.semibold(18.0), t.text_label);
    p.text(rect.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, body, t.font(13.0), t.text_dim);
}
