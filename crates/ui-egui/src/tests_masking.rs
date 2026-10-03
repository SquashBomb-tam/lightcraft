//! Headless tests of the Masking and Remove tools on the photo: overlay keys, pins, spot editing.

use std::time::Duration;

use lightcraft_develop::MaskShape;
use serde_json::json;

use crate::headless::Headless;
use crate::state::RightPanel;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn detail(panel: &str) -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": panel}), T);
    assert_eq!(r["ok"], true, "{r}");
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn pointer(h: &mut Headless, events: serde_json::Value) {
    let r = h.request("ui.pointer", json!({"events": events}), T);
    assert_eq!(r["ok"], true, "{r}");
}

fn develop(h: &Headless) -> lightcraft_develop::DevelopSettings {
    let id = h.app.session.active().expect("active photo");
    (*h.app.session.develop_of(id).unwrap_or_default()).clone()
}

#[test]
fn h_hides_and_shows_the_masks() {
    use lightcraft_pipeline::Overlay;
    let mut h = detail("panel.masking");
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.3, 0.4], "rx": 0.1, "ry": 0.1}));
    exec(&mut h, "mask.add", json!({"kind": "linear", "start": [0.7, 0.2], "end": [0.7, 0.6]}));
    exec(&mut h, "mask.adjust", json!({"values": {"exposure": 1.0}}));
    let before = develop(&h);
    assert_ne!(crate::panels::detail::view_overlay(&h.app, &before), Overlay::None);
    // H hides the overlay, pins, outlines and handles; the masks and their adjustments stay
    h.request("ui.key", json!({"key": "h"}), T);
    assert!(h.app.ui.mask_hidden);
    assert_eq!(h.app.ui.right, RightPanel::Masking, "H doesn't open Remove while masking");
    assert_eq!(crate::panels::detail::view_overlay(&h.app, &before), Overlay::None);
    assert_eq!(develop(&h), before, "hiding changes nothing in the edit");
    assert!(!h.request("ui.widgets", json!({"filter": "maskPin"}), T)["result"].to_string().contains("maskPin"), "no pins to grab");
    // hidden pins can't be clicked or dragged: the radial mask stays put and unselected
    pointer(
        &mut h,
        json!([{"kind": "down", "x": 0.3, "y": 0.4}, {"kind": "drag", "x": 0.4, "y": 0.5}, {"kind": "drag", "x": 0.5, "y": 0.6}, {"kind": "up", "x": 0.5, "y": 0.6}]),
    );
    assert_eq!(h.app.session.active_mask, Some(2));
    assert_eq!(develop(&h), before, "dragging over a hidden pin (or the photo) moves nothing");
    // H again shows them
    h.request("ui.key", json!({"key": "h"}), T);
    assert!(!h.app.ui.mask_hidden);
    assert_ne!(crate::panels::detail::view_overlay(&h.app, &before), Overlay::None);
    pointer(&mut h, json!([{"kind": "down", "x": 0.3, "y": 0.4}, {"kind": "up", "x": 0.3, "y": 0.4}]));
    assert_eq!(h.app.session.active_mask, Some(1), "pins are back");
    // the command: explicit hide/show, bad input refused
    assert_eq!(exec(&mut h, "view.maskHide", json!({"hide": true}))["maskHidden"], true);
    assert_eq!(exec(&mut h, "view.maskHide", json!({"hide": true}))["maskHidden"], true);
    let r = h.request("engine.execute", json!({"command": "view.maskHide", "params": {"hide": "yes"}}), T);
    assert_eq!(r["ok"], false, "{r}");
    assert!(h.app.ui.mask_hidden, "a refused call changes nothing");
    // the brush still paints while hidden
    exec(&mut h, "tool.brush", json!({}));
    pointer(&mut h, json!([{"kind": "down", "x": 0.2, "y": 0.8}, {"kind": "drag", "x": 0.25, "y": 0.8}, {"kind": "up", "x": 0.25, "y": 0.8}]));
    assert!(develop(&h).masks.iter().any(|m| m.components.iter().any(|c| matches!(c.shape, MaskShape::Brush { .. }))), "brush stroke added");
    // creating a mask from the panel shows the masks again (the new one is drawn on the photo)
    exec(&mut h, "view.maskHide", json!({"hide": true}));
    let r = h.request("ui.clickWidget", json!({"id": "maskNew:radial"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(!h.app.ui.mask_hidden);
    // leaving the Masking panel forgets the hidden state; outside it, H opens Remove as before
    exec(&mut h, "view.maskHide", json!({"hide": true}));
    exec(&mut h, "panel.masking", json!({}));
    assert_eq!(h.app.ui.right, RightPanel::None);
    assert!(!h.app.ui.mask_hidden);
    h.request("ui.key", json!({"key": "h"}), T);
    assert_eq!(h.app.ui.right, RightPanel::Remove);
    assert!(!h.app.ui.mask_hidden);
}

/// The first mask's radial gradient: (rx, ry, angle, feather).
fn radial(h: &Headless) -> (f64, f64, f64, f64) {
    let MaskShape::Radial { center, rx, ry, angle, feather, .. } = develop(h).masks[0].components[0].shape.clone() else { panic!("radial") };
    assert!((center.x - 0.5).abs() < 1e-9 && (center.y - 0.5).abs() < 1e-9, "the centre never moves: {center:?}");
    (rx, ry, angle, feather)
}

#[test]
fn radial_handles_resize_rotate_and_feather() {
    let mut h = detail("panel.masking");
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5], "rx": 0.2, "ry": 0.1, "angle": 0.0, "feather": 50.0}));
    h.request("ui.widgets", json!({}), T);
    let img = h.app.image_rect.expect("photo on screen");
    let long = img.width().max(img.height()) as f64;
    let drag = |h: &mut Headless, handle: u8, extra: serde_json::Value| {
        let mut p = json!({"id": format!("maskHandle:1:0:{handle}"), "steps": 8});
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let r = h.request("ui.dragWidget", p, T);
        assert_eq!(r["ok"], true, "{r}");
        h.request("ui.widgets", json!({}), T);
    };
    // the right handle widens the ellipse by the distance dragged; nothing else changes
    drag(&mut h, 3, json!({"dx": 60.0}));
    let (rx, ry, angle, feather) = radial(&h);
    assert!((rx - (0.2 + 60.0 / long)).abs() < 0.01, "rx {rx}, expected ≈ {}", 0.2 + 60.0 / long);
    assert!((ry - 0.1).abs() < 1e-9 && angle == 0.0 && feather == 50.0);
    // one drag is one undo step
    exec(&mut h, "edit.undo", json!({}));
    assert!((radial(&h).0 - 0.2).abs() < 1e-9, "undo restores the width");
    // the top handle with Shift scales both radii together
    drag(&mut h, 6, json!({"dy": -30.0, "shift": true}));
    let (rx, ry, ..) = radial(&h);
    assert!(ry > 0.1 + 20.0 / long, "taller: {ry}");
    assert!((rx / ry - 2.0).abs() < 0.02, "proportions kept: {rx} / {ry}");
    // the rotate knob, dragged to the right of the centre, turns the ellipse a quarter turn
    let c = img.center();
    drag(&mut h, 7, json!({"toX": c.x + 150.0, "toY": c.y}));
    let angle = radial(&h).2;
    assert!((angle - 90.0).abs() < 2.0, "angle {angle}");
    // the feather diamond dragged onto the edge makes a hard edge
    exec(
        &mut h,
        "mask.update",
        json!({"shape": {"kind": "radial", "center": {"x": 0.5, "y": 0.5}, "rx": 0.2, "ry": 0.1, "angle": 0.0, "feather": 50.0, "invert": false}}),
    );
    h.request("ui.widgets", json!({}), T);
    drag(&mut h, 8, json!({"toX": c.x + (0.25 * long) as f32, "toY": c.y}));
    assert!(radial(&h).3 < 1.0, "feather {}", radial(&h).3);
    // ... and the panel's Feather slider sets it too (dragged all the way right: fully soft)
    let r = h.request("ui.dragWidget", json!({"id": "slider:maskFeather", "fx": 0.5, "dx": 2000.0}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.request("ui.widgets", json!({}), T);
    assert!((radial(&h).3 - 100.0).abs() < 1e-9, "feather {}", radial(&h).3);
    // handles exist only on the selected mask: deselecting removes them
    exec(&mut h, "mask.select", json!({"id": null}));
    h.request("ui.widgets", json!({}), T);
    assert!(!h.request("ui.widgets", json!({"filter": "maskHandle"}), T)["result"].to_string().contains("maskHandle"));
}

#[test]
fn mask_overlay_keys_and_pins() {
    use lightcraft_pipeline::{MaskView, Overlay};
    let mut h = detail("panel.masking");
    assert_eq!(h.app.ui.right, RightPanel::Masking);
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.3, 0.4], "rx": 0.1, "ry": 0.1}));
    exec(&mut h, "mask.add", json!({"kind": "linear", "start": [0.7, 0.2], "end": [0.7, 0.6]}));
    assert_eq!(h.app.session.active_mask, Some(2));
    // the loupe asks the renderer for the selected mask's overlay
    let d = develop(&h);
    let o = crate::panels::detail::view_overlay(&h.app, &d);
    assert_eq!(o, Overlay::Mask { id: 2, view: MaskView::Color, color: [230, 30, 40], opacity: 50 });
    // O toggles it, Shift+O cycles the mode (and leaves the crop overlay alone)
    h.request("ui.key", json!({"key": "o"}), T);
    assert!(!h.app.ui.mask_overlay);
    assert_eq!(crate::panels::detail::view_overlay(&h.app, &d), Overlay::None);
    h.request("ui.key", json!({"key": "o"}), T);
    let crop = h.app.ui.crop_overlay;
    h.request("ui.key", json!({"key": "o", "shift": true}), T);
    assert_eq!(h.app.ui.mask_overlay_mode, "colorOnBw");
    assert_eq!(h.app.ui.crop_overlay, crop);
    exec(&mut h, "view.maskOverlayMode", json!({"mode": "whiteOnBlack"}));
    exec(&mut h, "view.maskOverlayColor", json!({"color": "#2870f0", "opacity": 80}));
    assert_eq!((h.app.ui.mask_overlay_color, h.app.ui.mask_overlay_opacity), ([0x28, 0x70, 0xf0], 80.0));
    let r = h.request("engine.execute", json!({"command": "view.maskOverlayMode", "params": {"mode": "nope"}}), T);
    assert_eq!(r["ok"], false, "{r}");
    // clicking another mask's pin selects that mask
    pointer(&mut h, json!([{"kind": "down", "x": 0.3, "y": 0.4}, {"kind": "up", "x": 0.3, "y": 0.4}]));
    assert_eq!(h.app.session.active_mask, Some(1));
    // dragging a pin moves its component (one undo step)
    pointer(
        &mut h,
        json!([{"kind": "down", "x": 0.3, "y": 0.4}, {"kind": "drag", "x": 0.35, "y": 0.45}, {"kind": "drag", "x": 0.5, "y": 0.6}, {"kind": "up", "x": 0.5, "y": 0.6}]),
    );
    let MaskShape::Radial { center, .. } = develop(&h).masks[0].components[0].shape.clone() else { panic!("radial") };
    assert!((center.x - 0.5).abs() < 0.02 && (center.y - 0.6).abs() < 0.02, "{center:?}");
    // dragging the linear gradient's pin (a non-selected mask) selects and moves it
    pointer(
        &mut h,
        json!([{"kind": "down", "x": 0.7, "y": 0.4}, {"kind": "drag", "x": 0.72, "y": 0.4}, {"kind": "drag", "x": 0.8, "y": 0.4}, {"kind": "up", "x": 0.8, "y": 0.4}]),
    );
    assert_eq!(h.app.session.active_mask, Some(2));
    let MaskShape::Linear { start, end } = develop(&h).masks[1].components[0].shape.clone() else { panic!("linear") };
    assert!((start.x - 0.8).abs() < 0.02 && (end.x - 0.8).abs() < 0.02 && (start.y - 0.2).abs() < 0.02, "{start:?} {end:?}");
    // hidden pins can't be grabbed
    exec(&mut h, "view.maskPins", json!({"show": false}));
    pointer(&mut h, json!([{"kind": "down", "x": 0.5, "y": 0.6}, {"kind": "up", "x": 0.5, "y": 0.6}]));
    assert_eq!(h.app.session.active_mask, Some(2));
    h.settle(SETTLE);
}

#[test]
fn brush_strokes_carry_auto_mask() {
    let mut h = detail("panel.masking");
    let r = h.request("ui.set", json!({"brushAutoMask": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    exec(&mut h, "tool.brush", json!({}));
    assert_eq!(h.app.ui.tool, "brush");
    pointer(
        &mut h,
        json!([{"kind": "down", "x": 0.3, "y": 0.5}, {"kind": "drag", "x": 0.4, "y": 0.5}, {"kind": "drag", "x": 0.5, "y": 0.5}, {"kind": "up", "x": 0.5, "y": 0.5}]),
    );
    let d = develop(&h);
    let MaskShape::Brush { strokes } = &d.masks[0].components[0].shape else { panic!("brush") };
    assert!(strokes[0].auto_mask && strokes[0].points.len() >= 2, "{strokes:?}");
    assert!(!strokes[0].erase);
    // holding ⌥ paints an erase stroke without switching the brush to Erase
    let r = h.request(
        "ui.pointer",
        json!({"events": [{"kind": "down", "x": 0.35, "y": 0.5}, {"kind": "drag", "x": 0.45, "y": 0.5}, {"kind": "up", "x": 0.45, "y": 0.5}], "alt": true}),
        T,
    );
    assert_eq!(r["ok"], true, "{r}");
    let d = develop(&h);
    let MaskShape::Brush { strokes } = &d.masks[0].components[0].shape else { panic!("brush") };
    assert_eq!(strokes.len(), 2, "{strokes:?}");
    assert!(strokes[1].erase, "⌥ erases");
    assert!(!h.app.ui.brush_erase, "the brush mode is unchanged");
    h.settle(SETTLE);
}

#[test]
fn remove_spots_by_pointer_and_keyboard() {
    let mut h = detail("panel.remove");
    assert_eq!(h.app.ui.right, RightPanel::Remove);
    let spots = |h: &Headless| develop(h).spots;
    // paint a spot: it's added with the brush's size/feather/opacity and selected
    let r = h.request("ui.set", json!({"removeFeather": 30.0, "removeOpacity": 80.0}), T);
    assert_eq!(r["ok"], true, "{r}");
    pointer(&mut h, json!([{"kind": "down", "x": 0.3, "y": 0.6}, {"kind": "up", "x": 0.3, "y": 0.6}]));
    assert_eq!(spots(&h).len(), 1);
    assert_eq!((spots(&h)[0].feather, spots(&h)[0].opacity), (30.0, 80.0));
    assert_eq!(h.app.session.active_spot, Some(0));
    // [ / ] resize the selected spot, Shift+[ / Shift+] feather it
    let s0 = spots(&h)[0].size;
    h.request("ui.key", json!({"key": "]"}), T);
    assert!(spots(&h)[0].size > s0 * 1.1, "{} vs {s0}", spots(&h)[0].size);
    h.request("ui.key", json!({"key": "["}), T);
    h.request("ui.key", json!({"key": "["}), T);
    assert!(spots(&h)[0].size < s0 * 0.9);
    h.request("ui.key", json!({"key": "[", "shift": true}), T);
    assert_eq!(spots(&h)[0].feather, 20.0);
    h.request("ui.key", json!({"key": "]", "shift": true}), T);
    assert_eq!(spots(&h)[0].feather, 30.0);
    // / picks another source (and leaves the filmstrip alone)
    let (src, film) = (spots(&h)[0].source_offset, h.app.ui.filmstrip);
    h.request("ui.key", json!({"key": "/"}), T);
    assert_ne!(spots(&h)[0].source_offset, src);
    assert_eq!(h.app.ui.filmstrip, film);
    // a second spot elsewhere; clicking the first one's pin selects it
    pointer(&mut h, json!([{"kind": "down", "x": 0.7, "y": 0.3}, {"kind": "up", "x": 0.7, "y": 0.3}]));
    assert_eq!((spots(&h).len(), h.app.session.active_spot), (2, Some(1)));
    pointer(&mut h, json!([{"kind": "down", "x": 0.3, "y": 0.6}, {"kind": "up", "x": 0.3, "y": 0.6}]));
    assert_eq!((spots(&h).len(), h.app.session.active_spot), (2, Some(0)));
    // drag its target: it moves, its source offset stays
    let src = spots(&h)[0].source_offset;
    pointer(
        &mut h,
        json!([{"kind": "down", "x": 0.3, "y": 0.6}, {"kind": "drag", "x": 0.32, "y": 0.6}, {"kind": "drag", "x": 0.4, "y": 0.6}, {"kind": "up", "x": 0.4, "y": 0.6}]),
    );
    let sp = spots(&h)[0].clone();
    assert!((sp.points[0].x - 0.4).abs() < 0.01 && (sp.points[0].y - 0.6).abs() < 0.01, "{:?}", sp.points);
    assert_eq!(sp.source_offset, src);
    // drag its source to a fixed place
    let o = sp.source_offset.unwrap();
    let (sx, sy) = (sp.points[0].x + o.x, sp.points[0].y + o.y);
    pointer(
        &mut h,
        json!([{"kind": "down", "x": sx, "y": sy}, {"kind": "drag", "x": sx + 0.01, "y": sy}, {"kind": "drag", "x": 0.5, "y": 0.8}, {"kind": "up", "x": 0.5, "y": 0.8}]),
    );
    let sp = spots(&h)[0].clone();
    let o = sp.source_offset.unwrap();
    assert!((sp.points[0].x + o.x - 0.5).abs() < 0.01 && (sp.points[0].y + o.y - 0.8).abs() < 0.01, "{o:?}");
    // ⌫ deletes the selected spot, not the photo
    let photos = h.app.session.visible_cloned().len();
    h.request("ui.key", json!({"key": "delete"}), T);
    assert_eq!((spots(&h).len(), h.app.session.active_spot), (1, None));
    assert_eq!(h.app.session.visible_cloned().len(), photos);
    h.request("ui.key", json!({"key": "delete"}), T);
    assert_eq!(spots(&h).len(), 1, "nothing selected: nothing deleted");
    h.settle(SETTLE);
}

/// Masks list: double-click renames in place, the hover eye hides a mask, the overlay colour
/// cycles through the swatches.
#[test]
fn mask_list_rename_hide_and_overlay_colour() {
    let mut h = detail("panel.masking");
    exec(&mut h, "mask.add", json!({"kind": "linear"}));
    exec(&mut h, "mask.add", json!({"kind": "radial"}));
    h.settle(SETTLE);
    let first = develop(&h).masks[0].id;
    let r = h.request("ui.clickWidget", json!({"id": format!("mask:{first}"), "count": 2}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(Duration::from_secs(5));
    assert_eq!(h.app.ui.renaming_mask.as_ref().map(|r| r.0), Some(first), "double-click starts renaming");
    h.request("ui.key", json!({"key": "A", "cmd": true}), T);
    h.request("ui.text", json!({"text": "Sky"}), T);
    h.request("ui.key", json!({"key": "Enter"}), T);
    h.settle(Duration::from_secs(5));
    assert!(h.app.ui.renaming_mask.is_none());
    assert_eq!(develop(&h).masks[0].name, "Sky");
    // hover the row, then click its eye
    let r = h.request("ui.hoverWidget", json!({"id": format!("mask:{first}")}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(Duration::from_secs(5));
    let r = h.request("ui.clickWidget", json!({"id": format!("maskVisible:{first}")}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(!develop(&h).masks[0].visible);
    // overlay colour: no params = next swatch
    let before = h.app.ui.mask_overlay_color;
    exec(&mut h, "view.maskOverlayColor", json!({}));
    let all = crate::panels::masking::OVERLAY_COLORS;
    let i = all.iter().position(|c| *c == before).unwrap();
    assert_eq!(h.app.ui.mask_overlay_color, all[(i + 1) % all.len()]);
}
