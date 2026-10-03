//! The Masking panel: create masks, list them, edit components and local adjustments.

use egui::{Align2, Rect, Sense, Stroke, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{ControlSpec, LocalAdjustments, MaskShape, Section, Track};
use serde_json::json;

use super::edit::apply_slider_out;
use super::right::header;
use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{divider, icon_button, register, slider, text_button};

const fn spec(id: &'static str, label: &'static str, min: f64, max: f64, step: f64, decimals: u8, track: Track) -> ControlSpec {
    ControlSpec { id, label, section: Section::Light, min, max, default: 0.0, step, decimals, track }
}

/// Local adjustment sliders (key = `LocalAdjustments` field name).
pub const LOCAL: &[ControlSpec] = &[
    spec("temp", "Temp", -100.0, 100.0, 1.0, 0, Track::Temp),
    spec("tint", "Tint", -100.0, 100.0, 1.0, 0, Track::Tint),
    spec("exposure", "Exposure", -4.0, 4.0, 0.01, 2, Track::Centered),
    spec("contrast", "Contrast", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("highlights", "Highlights", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("shadows", "Shadows", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("whites", "Whites", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("blacks", "Blacks", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("texture", "Texture", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("clarity", "Clarity", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("dehaze", "Dehaze", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("hue", "Hue", -100.0, 100.0, 1.0, 0, Track::Rainbow),
    spec("saturation", "Saturation", -100.0, 100.0, 1.0, 0, Track::Gradient { from: "#7a7a7a", to: "#e04a3a" }),
    spec("sharpness", "Sharpness", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("noise", "Noise", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("moire", "Moiré", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("defringe", "Defringe", -100.0, 100.0, 1.0, 0, Track::Centered),
];

pub fn local_get(a: &LocalAdjustments, key: &str) -> f64 {
    serde_json::to_value(a).ok().and_then(|v| v.get(key).and_then(|x| x.as_f64())).unwrap_or(0.0)
}

fn kind_label(s: &MaskShape) -> (&'static str, Icon) {
    match s {
        MaskShape::Brush { .. } => ("Brush", Icon::Brush),
        MaskShape::Linear { .. } => ("Linear Gradient", Icon::Linear),
        MaskShape::Radial { .. } => ("Radial Gradient", Icon::Radial),
        MaskShape::ColorRange { .. } => ("Color Range", Icon::Picker),
        MaskShape::LuminanceRange { .. } => ("Luminance Range", Icon::Sliders),
        MaskShape::DepthRange { .. } => ("Depth Range", Icon::Sliders),
        MaskShape::Subject => ("Subject", Icon::Subject),
        MaskShape::Sky => ("Sky", Icon::Sky),
        MaskShape::Background => ("Background", Icon::Subject),
        MaskShape::Object { .. } => ("Object", Icon::Subject),
        MaskShape::People { .. } => ("People", Icon::Subject),
        MaskShape::Landscape { .. } => ("Landscape", Icon::Sky),
    }
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let t = Tokens::get(ui.ctx());
    let d = app.session.develop_of(id).unwrap_or_default();
    header(ui, "Masking");
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 10 }).show(ui, |ui| {
        ui.label(egui::RichText::new("Create New Mask").color(t.text_dim));
        ui.add_space(6.0);
        let tiles: [(&str, &str, Icon); 8] = [
            ("subject", "Subject", Icon::Subject),
            ("sky", "Sky", Icon::Sky),
            ("background", "Background", Icon::Subject),
            ("brush", "Brush", Icon::Brush),
            ("linear", "Linear", Icon::Linear),
            ("radial", "Radial", Icon::Radial),
            ("luminanceRange", "Luminance", Icon::Sliders),
            ("colorRange", "Color", Icon::Picker),
        ];
        egui::Grid::new("mask-tiles").spacing(vec2(6.0, 6.0)).show(ui, |ui| {
            for (i, (kind, label, icon)) in tiles.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::click());
                register(ui.ctx(), format!("maskNew:{kind}"), r);
                ui.painter().rect_filled(r, 4.0, if resp.hovered() { t.hover } else { t.inset });
                paint(ui.painter(), Rect::from_center_size(r.center() - vec2(0.0, 7.0), vec2(20.0, 20.0)), *icon, t.text_label);
                ui.painter().text(pos2(r.center().x, r.bottom() - 9.0), Align2::CENTER_CENTER, *label, t.font(10.5), t.text_dim);
                if resp.clicked() {
                    // a new mask is drawn on the photo, so masks hidden with H show again
                    app.ui.mask_hidden = false;
                    match *kind {
                        "brush" | "linear" | "radial" => {
                            app.ui.tool = kind.to_string();
                            if *kind != "brush" {
                                let _ = app.run("mask.add", json!({"kind": kind}));
                            }
                        }
                        k => {
                            let _ = app.run("mask.add", json!({"kind": k}));
                        }
                    }
                }
                if i % 4 == 3 {
                    ui.end_row();
                }
            }
        });
    });
    divider(ui);
    // mask list
    egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 16, top: 8, bottom: 8 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Masks").font(t.semibold(13.0)).color(t.text_label));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let on = app.ui.mask_overlay;
                if icon_button(ui, "maskOverlay", Icon::Eye, vec2(24.0, 24.0), on, true, "Show overlay (O)").clicked() {
                    app.ui.mask_overlay = !on;
                }
            });
        });
        if d.masks.is_empty() {
            ui.label(egui::RichText::new("No masks yet. Choose a mask type above.").color(t.text_dim));
        }
        let count = d.masks.len();
        for (index, m) in d.masks.iter().enumerate() {
            let sel = app.session.active_mask == Some(m.id);
            if let Some((rid, name)) = app.ui.renaming_mask.as_mut().filter(|(rid, _)| *rid == m.id) {
                // inline rename: Enter (or leaving the field) commits, Escape cancels
                let rid = *rid;
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(ui.available_width()).id_salt(("maskRename", rid)));
                register(ui.ctx(), format!("maskRename:{rid}"), r.rect);
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    app.ui.renaming_mask = None;
                } else if r.lost_focus() {
                    let name = name.trim().to_string();
                    app.ui.renaming_mask = None;
                    if !name.is_empty() && name != m.name {
                        let _ = app.run("mask.rename", json!({"id": rid, "name": name}));
                    }
                }
                continue;
            }
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
            register(ui.ctx(), format!("mask:{}", m.id), r);
            ui.painter().rect_filled(
                r,
                4.0,
                if sel {
                    t.tool_active
                } else if resp.hovered() {
                    t.hover.gamma_multiply(0.7)
                } else {
                    t.chrome
                },
            );
            let icon = m.components.first().map(|c| kind_label(&c.shape).1).unwrap_or(Icon::Mask);
            paint(ui.painter(), Rect::from_min_size(r.min + vec2(8.0, 7.0), vec2(16.0, 16.0)), icon, t.text_label);
            ui.painter().text(
                pos2(r.left() + 32.0, r.center().y),
                Align2::LEFT_CENTER,
                &m.name,
                t.font(13.0),
                if m.visible { t.text } else { t.text_disabled },
            );
            // show / hide on hover (and always while hidden)
            // (the pointer test, not `hovered`: over the eye, the row itself no longer counts as hovered)
            if ui.rect_contains_pointer(r) || !m.visible {
                let er = Rect::from_center_size(pos2(r.right() - 16.0, r.center().y), vec2(22.0, 22.0));
                register(ui.ctx(), format!("maskVisible:{}", m.id), er);
                let eye = ui.interact(er, egui::Id::new(("maskVisible", m.id)), Sense::click());
                paint(
                    ui.painter(),
                    er.shrink(3.0),
                    if m.visible { Icon::Eye } else { Icon::EyeOff },
                    if eye.hovered() { t.text } else { t.text_dim },
                );
                if eye.clicked() {
                    let _ = app.run("mask.visible", json!({"id": m.id}));
                }
            }
            if resp.clicked() {
                let _ = app.run("mask.select", json!({"id": m.id}));
            }
            if resp.double_clicked() {
                app.ui.renaming_mask = Some((m.id, m.name.clone()));
            }
            resp.context_menu(|ui| mask_menu(app, ui, m.id, &m.name, m.visible, index, count));
        }
        if !d.masks.is_empty() {
            ui.add_space(6.0);
            overlay_options(app, ui);
        }
    });
    let Some(mid) = app.session.active_mask else { return };
    let Some(m) = d.masks.iter().find(|m| m.id == mid).cloned() else { return };
    divider(ui);
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 8, bottom: 8 }).show(ui, |ui| {
        for (i, c) in m.components.iter().enumerate() {
            let (label, icon) = kind_label(&c.shape);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                paint(ui.painter(), r, icon, t.text_label);
                let op = match c.op {
                    lightcraft_develop::MaskOp::Add => "",
                    lightcraft_develop::MaskOp::Subtract => "− ",
                    lightcraft_develop::MaskOp::Intersect => "∩ ",
                };
                ui.label(format!("{op}{label}{}", if c.invert { " (inverted)" } else { "" }));
            });
            // a radial gradient's Feather (also the diamond handle on the photo)
            if let MaskShape::Radial { feather, .. } = c.shape {
                let spec = ControlSpec { default: 50.0, ..spec("maskFeather", "Feather", 0.0, 100.0, 1.0, 0, Track::Plain) };
                let out = ui.push_id(("maskFeather", i), |ui| slider(ui, &spec, feather, true, None)).inner;
                let shape = c.shape.clone();
                apply_slider_out(app, &spec, out, |app, v| {
                    let MaskShape::Radial { center, rx, ry, angle, invert, .. } = shape.clone() else { return Ok(json!(null)) };
                    let feather = v.clamp(0.0, 100.0);
                    app.run("mask.update", json!({"component": i, "shape": MaskShape::Radial { center, rx, ry, angle, feather, invert }}))
                });
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let add = text_button(ui, "maskAddComp", "Add", false);
            egui::Popup::menu(&add).show(|ui| component_menu(app, ui, "add"));
            let sub = text_button(ui, "maskSubComp", "Subtract", false);
            egui::Popup::menu(&sub).show(|ui| component_menu(app, ui, "subtract"));
            if text_button(ui, "maskInvert", "Invert", m.invert).clicked() {
                let _ = app.run("mask.invert", json!({}));
            }
            if icon_button(ui, "maskDelete", Icon::Trash, vec2(26.0, 24.0), false, true, "Delete mask").clicked() {
                let _ = app.run("mask.delete", json!({}));
            }
        });
    });
    if app.ui.tool == "brush" {
        divider(ui);
        brush_settings(app, ui);
    }
    divider(ui);
    for s in LOCAL {
        let v = local_get(&m.adjust, s.id);
        let out = slider(ui, s, v, true, None);
        apply_slider_out(app, s, out, |app, v| app.run("mask.adjust", json!({"values": {s.id: v}})));
    }
    let amt = ControlSpec {
        id: "amount",
        label: "Amount",
        section: Section::Light,
        min: 0.0,
        max: 200.0,
        default: 100.0,
        step: 1.0,
        decimals: 0,
        track: Track::Plain,
    };
    let out = slider(ui, &amt, m.adjust.amount, true, None);
    apply_slider_out(app, &amt, out, |app, v| app.run("mask.adjust", json!({"values": {"amount": v}})));
    ui.add_space(30.0);
    let _ = Stroke::NONE;
}

/// The right-click menu of a mask in the Masks list.
fn mask_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, id: u32, name: &str, visible: bool, index: usize, count: usize) {
    let mut run = |ui: &mut egui::Ui, label: &str, enabled: bool, cmd: &str, p: serde_json::Value| {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            let _ = app.run(cmd, p);
            ui.close();
        }
    };
    run(ui, "Duplicate Mask", true, "mask.duplicate", json!({"id": id}));
    run(ui, "Duplicate and Invert Mask", true, "mask.duplicate", json!({"id": id, "invert": true}));
    run(ui, "Invert Mask", true, "mask.invert", json!({"id": id}));
    run(ui, if visible { "Hide Mask" } else { "Show Mask" }, true, "mask.visible", json!({"id": id}));
    ui.separator();
    run(ui, "Move Up", index > 0, "mask.move", json!({"id": id, "delta": -1}));
    run(ui, "Move Down", index + 1 < count, "mask.move", json!({"id": id, "delta": 1}));
    ui.separator();
    if ui.button("Rename…").clicked() {
        app.ui.renaming_mask = Some((id, name.to_string()));
        ui.close();
    }
    if ui.button("Delete Mask").clicked() {
        let _ = app.run("mask.delete", json!({"id": id}));
        ui.close();
    }
}

/// Overlay colours offered as swatches (the colour of the selected one is used for the tint).
pub const OVERLAY_COLORS: [[u8; 3]; 5] = [[230, 30, 40], [40, 200, 70], [40, 110, 240], [250, 210, 30], [255, 255, 255]];

/// How the selected mask is shown: overlay mode, colour, opacity, pins.
fn overlay_options(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    use lightcraft_pipeline::MaskView;
    let t = Tokens::get(ui.ctx());
    let view = MaskView::parse(&app.ui.mask_overlay_mode).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Overlay").color(t.text_dim));
        let r = crate::widgets::dropdown(ui, "maskOverlayMode", view.label(), t.font(12.5), t.text_label);
        egui::Popup::menu(&r).show(|ui| {
            for v in MaskView::ALL {
                if ui.selectable_label(v == view, v.label()).clicked() {
                    let _ = app.run("view.maskOverlayMode", json!({"mode": v.name()}));
                }
            }
        });
    });
    let colored = matches!(view, MaskView::Color | MaskView::ColorOnBw);
    if colored {
        ui.horizontal(|ui| {
            for c in OVERLAY_COLORS {
                let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                register(ui.ctx(), format!("maskOverlayColor:{:02x}{:02x}{:02x}", c[0], c[1], c[2]), r);
                ui.painter().rect_filled(r.shrink(2.0), 3.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
                if app.ui.mask_overlay_color == c {
                    ui.painter().rect_stroke(r, 3.0, Stroke::new(1.5, t.text), egui::StrokeKind::Inside);
                }
                if resp.clicked() {
                    let _ = app.run("view.maskOverlayColor", json!({"color": c}));
                }
            }
        });
    }
    let spec = ControlSpec {
        id: "ui.maskOverlayOpacity",
        label: "Opacity",
        section: Section::Light,
        min: 0.0,
        max: 100.0,
        default: 50.0,
        step: 1.0,
        decimals: 0,
        track: Track::Plain,
    };
    let out = slider(ui, &spec, app.ui.mask_overlay_opacity as f64, colored && app.ui.mask_overlay, None);
    if let Some(v) = out.value {
        app.ui.mask_overlay_opacity = v as f32;
    }
    let mut pins = app.ui.mask_pins;
    if ui.checkbox(&mut pins, "Show Pins").changed() {
        let _ = app.run("view.maskPins", json!({"show": pins}));
    }
}

fn component_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, op: &str) {
    for (kind, label) in [
        ("brush", "Brush"),
        ("linear", "Linear Gradient"),
        ("radial", "Radial Gradient"),
        ("sky", "Sky"),
        ("subject", "Subject"),
        ("luminanceRange", "Luminance Range"),
    ] {
        if ui.button(label).clicked() {
            if kind == "brush" {
                app.ui.tool = "brush".into();
                app.ui.brush_erase = op == "subtract";
            } else {
                let _ = app.run("mask.addComponent", json!({"op": op, "kind": kind}));
            }
        }
    }
}

fn brush_settings(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 6, bottom: 0 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Brush").font(t.semibold(13.0)));
            if text_button(ui, "brushAdd", "Add", !app.ui.brush_erase).clicked() {
                app.ui.brush_erase = false;
            }
            if text_button(ui, "brushErase", "Erase", app.ui.brush_erase).clicked() {
                app.ui.brush_erase = true;
            }
        });
        let mut auto = app.ui.brush_auto_mask;
        if ui.checkbox(&mut auto, "Auto Mask").on_hover_text("Paint only areas like the one under the brush").changed() {
            app.ui.brush_auto_mask = auto;
        }
    });
    for (id, label, min, max, get) in [
        ("ui.brushSize", "Size", 1.0, 100.0, (app.ui.brush_size * 400.0) as f64),
        ("ui.brushFeather", "Feather", 0.0, 100.0, app.ui.brush_feather as f64),
        ("ui.brushFlow", "Flow", 1.0, 100.0, app.ui.brush_flow as f64),
    ] {
        let s = ControlSpec { id, label, section: Section::Light, min, max, default: min, step: 1.0, decimals: 0, track: Track::Plain };
        let out = slider(ui, &s, get, true, None);
        if let Some(v) = out.value {
            match id {
                "ui.brushSize" => app.ui.brush_size = (v / 400.0) as f32,
                "ui.brushFeather" => app.ui.brush_feather = v as f32,
                _ => app.ui.brush_flow = v as f32,
            }
        }
    }
}
