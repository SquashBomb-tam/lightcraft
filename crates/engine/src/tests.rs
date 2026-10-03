use serde_json::{Value, json};

use crate::{Session, command_specs};

fn demo() -> Session {
    Session::with_demo()
}

fn active_dev(s: &Session) -> lightcraft_develop::DevelopSettings {
    (*s.develop_of(s.active().unwrap()).unwrap()).clone()
}

#[test]
fn demo_library_loads() {
    let mut s = demo();
    assert_eq!(s.visible().len(), 24);
    assert!(s.active().is_some());
    let st = s.execute("catalog.stats", &json!({})).unwrap();
    assert_eq!(st["photos"], 24);
    let albums = s.execute("albums.list", &json!({})).unwrap();
    assert!(albums.as_array().unwrap().iter().any(|a| a["folder"] == true));
}

#[test]
fn rating_flag_undo_redo() {
    let mut s = demo();
    let id = s.active().unwrap();
    s.execute("photo.rate", &json!({"rating": 5})).unwrap();
    s.execute("photo.flag", &json!({"flag": "reject"})).unwrap();
    assert_eq!(s.catalog.photo(id).unwrap().rating, 5);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_ne!(s.catalog.photo(id).unwrap().flag, lightcraft_catalog::Flag::Reject);
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.catalog.photo(id).unwrap().flag, lightcraft_catalog::Flag::Reject);
    assert!(s.execute("photo.rate", &json!({"rating": 7})).is_err());
}

#[test]
fn develop_set_and_interaction_coalesces() {
    let mut s = demo();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    assert_eq!(active_dev(&s).light.exposure, 0.5);
    let undo_before = s.undo.len();
    s.execute("develop.beginInteraction", &json!({"label": "Exposure"})).unwrap();
    for v in [0.6, 0.8, 1.2] {
        s.execute("develop.set", &json!({"control": "light.exposure", "value": v})).unwrap();
    }
    s.execute("develop.endInteraction", &json!({})).unwrap();
    assert_eq!(s.undo.len(), undo_before + 1, "a drag is one undo step");
    assert_eq!(active_dev(&s).light.exposure, 1.2);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(active_dev(&s).light.exposure, 0.5);
    // unknown control and bad values are rejected
    assert!(s.execute("develop.set", &json!({"control": "nope", "value": 1})).is_err());
    assert!(s.execute("develop.set", &json!({"values": {"light.contrast": "x"}})).is_err());
    // clamped
    s.execute("develop.set", &json!({"values": {"light.contrast": 500}})).unwrap();
    assert_eq!(active_dev(&s).light.contrast, 100.0);
}

#[test]
fn copy_paste_sync_presets() {
    let mut s = demo();
    let vis = s.visible_cloned();
    s.execute("develop.set", &json!({"values": {"effects.clarity": 30, "light.shadows": 20}})).unwrap();
    s.execute("develop.copy", &json!({})).unwrap();
    s.execute("library.select", &json!({"ids": [vis[1].0, vis[2].0]})).unwrap();
    s.execute("develop.paste", &json!({})).unwrap();
    assert_eq!(s.develop_of(vis[2]).unwrap().effects.clarity, 30.0);
    s.execute("preset.apply", &json!({"id": "lc.bw-high-contrast", "amount": 100})).unwrap();
    assert_eq!(s.develop_of(vis[1]).unwrap().treatment, lightcraft_develop::Treatment::Bw);
    let r = s.execute("preset.create", &json!({"name": "Mine", "groups": ["light", "effects"]})).unwrap();
    assert!(r["id"].as_str().unwrap().starts_with("user."));
    assert!(s.execute("preset.delete", &json!({"id": "lc.moody"})).is_err());
}

#[test]
fn albums_crud() {
    let mut s = demo();
    let f = s.execute("album.create", &json!({"name": "Trips", "folder": true})).unwrap()["id"].as_u64().unwrap();
    let a = s.execute("album.create", &json!({"name": "Best", "parent": f, "addSelected": true})).unwrap()["id"].as_u64().unwrap();
    s.execute("library.source", &json!({"kind": "album", "id": a})).unwrap();
    assert_eq!(s.visible().len(), 1);
    s.execute("album.rename", &json!({"id": a, "name": "Best of"})).unwrap();
    s.execute("library.source", &json!({"kind": "all"})).unwrap();
    s.execute("library.selectAll", &json!({})).unwrap();
    s.execute("album.addPhotos", &json!({"id": a})).unwrap();
    assert_eq!(s.catalog.album(lightcraft_catalog::AlbumId(a)).unwrap().photos.len(), 24);
    assert!(s.execute("album.delete", &json!({"id": f})).is_err(), "non-empty folder");
    s.execute("album.delete", &json!({"id": a})).unwrap();
    s.execute("album.delete", &json!({"id": f})).unwrap();
}

#[test]
fn filters_and_delete_restore() {
    let mut s = demo();
    s.execute("library.filter", &json!({"rating": 4})).unwrap();
    let n = s.visible().len();
    assert!(n > 0 && n < 24);
    s.execute("library.clearFilter", &json!({})).unwrap();
    s.execute("photo.delete", &json!({})).unwrap();
    assert_eq!(s.visible().len(), 23);
    s.execute("library.source", &json!({"kind": "recentlyDeleted"})).unwrap();
    assert_eq!(s.visible().len(), 1);
    s.execute("photo.restore", &json!({})).unwrap();
    s.execute("library.source", &json!({"kind": "all"})).unwrap();
    assert_eq!(s.visible().len(), 24);
}

#[test]
fn masks_crop_and_render() {
    let mut s = demo();
    s.execute("mask.add", &json!({"kind": "radial", "center": [0.5, 0.5]})).unwrap();
    s.execute("mask.adjust", &json!({"values": {"exposure": 1.0}})).unwrap();
    s.execute("mask.brushStroke", &json!({"points": [[0.2, 0.2], [0.3, 0.3]], "size": 0.05})).unwrap();
    let d = active_dev(&s);
    assert_eq!(d.masks.len(), 1);
    assert_eq!(d.masks[0].components.len(), 2);
    s.execute("crop.aspect", &json!({"aspect": "1x1"})).unwrap();
    s.execute("crop.straighten", &json!({"angle": 5.0})).unwrap();
    let id = s.active().unwrap();
    let r = s.render_now(id, 200, 200).unwrap();
    assert_eq!((r.image.width, r.image.height), (200, 200));
    s.execute("develop.auto", &json!({})).unwrap();
    s.execute("develop.wb", &json!({"mode": "auto"})).unwrap();
    s.execute("develop.wbPick", &json!({"x": 0.5, "y": 0.5})).unwrap();
}

#[test]
fn versions_history_and_reset() {
    let mut s = demo();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    s.execute("version.create", &json!({"name": "bright"})).unwrap();
    s.execute("develop.reset", &json!({})).unwrap();
    assert!(active_dev(&s).is_unedited());
    s.execute("version.restore", &json!({"index": 0})).unwrap();
    assert_eq!(active_dev(&s).light.exposure, 1.0);
    let h = s.execute("history.list", &json!({})).unwrap();
    assert!(h["history"].as_array().unwrap().len() >= 3);
}

#[test]
fn op_log_replay_reproduces_catalog() {
    let mut s = demo();
    let snap = s.catalog.to_snapshot();
    let _ = s.drain_log();
    s.execute("photo.rate", &json!({"rating": 2})).unwrap();
    s.execute("develop.set", &json!({"control": "effects.dehaze", "value": 40})).unwrap();
    s.execute("album.create", &json!({"name": "X"})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    let log: String = s.drain_log().iter().map(lightcraft_catalog::Catalog::op_to_log_line).collect();
    let mut c = lightcraft_catalog::Catalog::from_snapshot(&snap).unwrap();
    c.replay(&log).unwrap();
    assert_eq!(c.to_snapshot(), s.catalog.to_snapshot());
}

/// Every command runs (or fails cleanly with an error) with empty params and has metadata.
#[test]
fn command_sweep() {
    let mut s = demo();
    let mut ids = std::collections::HashSet::new();
    for c in command_specs() {
        assert!(ids.insert(c.id), "duplicate {}", c.id);
        assert!(!c.label.is_empty() && !c.params.is_empty(), "{}", c.id);
        let _ = s.execute(c.id, &Value::Null);
    }
    assert!(s.commands().len() >= 70, "{}", s.commands().len());
}

#[test]
fn upright_and_guides_commands() {
    let mut s = demo();
    let r = s.execute("geometry.upright", &json!({"mode": "level"})).unwrap();
    assert_eq!(r["mode"], "level");
    let d = active_dev(&s);
    assert_eq!(d.geometry.upright, lightcraft_develop::Upright::Level);
    assert!(d.geometry.upright_transform.is_some(), "analysis result is stored");
    s.execute("geometry.upright", &json!({"mode": "off"})).unwrap();
    assert!(active_dev(&s).geometry.upright_transform.is_none());
    for i in 0..5 {
        let x = 0.1 + 0.15 * i as f64;
        s.execute("geometry.guides", &json!({"guides": [[x, 0.2, x + 0.02, 0.8]], "add": true})).unwrap();
    }
    let d = active_dev(&s);
    assert_eq!(d.geometry.upright, lightcraft_develop::Upright::Guided);
    assert_eq!(d.geometry.guides.len(), 4, "at most four guides");
    assert!((d.geometry.guides[0].0.x - 0.25).abs() < 1e-9, "oldest dropped");
    let id = s.active().unwrap();
    assert!(s.render_now(id, 160, 160).is_ok());
    assert!(s.execute("geometry.upright", &json!({"mode": "sideways"})).is_err());
}

#[test]
fn memory_report_counts_decoded_sources_and_renders() {
    let mut s = demo();
    let r = s.execute("library.memory", &json!({})).unwrap();
    assert_eq!(r["previewSources"]["count"], 0);
    assert!(r["gpu"]["allocated"].is_u64() && r.get("engineBytes").is_some());
    let id = s.active().unwrap();
    s.render_now(id, 800, 600).unwrap();
    let m = s.memory_report();
    assert_eq!(m.preview_sources.count, 1);
    // a 2560 px linear float source: 12 bytes per pixel
    assert!(m.preview_sources.bytes >= 12 * 2560 * 1000, "{:?}", m.preview_sources);
    assert_eq!(m.engine_bytes, m.thumb_sources.bytes + m.preview_sources.bytes + m.full_source.bytes + m.rendered.bytes);
    let r = s.thumb_job(id, 200).unwrap().run();
    s.accept(&r);
    let m = s.memory_report();
    assert_eq!(m.thumb_sources.count, 1);
    assert!(m.rendered.count >= 1, "the thumbnail render is cached");
}

#[test]
fn paste_selected_settings_pastes_only_chosen_copied_groups() {
    let mut s = demo();
    let ids: Vec<_> = s.visible().iter().copied().take(2).collect();
    s.execute("library.select", &json!({"ids": [ids[0].0]})).unwrap();
    s.execute("develop.set", &json!({"values": {"light.exposure": 1.0, "color.vibrance": 30, "detail.sharpenAmount": 90}})).unwrap();
    s.execute("develop.copy", &json!({"groups": ["light", "color"]})).unwrap();
    // `detail` was not copied: asking for it pastes nothing for it
    s.execute("develop.paste", &json!({"ids": [ids[1].0], "groups": ["light", "detail"]})).unwrap();
    let d = s.develop_of(ids[1]).unwrap();
    let base = lightcraft_develop::DevelopSettings::default();
    assert_eq!(d.light.exposure, 1.0);
    assert_eq!(d.color.vibrance, base.color.vibrance);
    assert_ne!(d.detail.sharpen_amount, 90.0);
    // without groups: everything copied
    s.execute("develop.paste", &json!({"ids": [ids[1].0]})).unwrap();
    assert_eq!(s.develop_of(ids[1]).unwrap().color.vibrance, 30.0);
}

#[test]
fn masks_reorder_and_duplicate_and_invert() {
    let mut s = demo();
    for kind in ["linear", "radial", "brush"] {
        s.execute("mask.add", &json!({"kind": kind})).unwrap();
    }
    let ids = |s: &Session| active_dev(s).masks.iter().map(|m| m.id).collect::<Vec<_>>();
    let [a, b, c] = ids(&s)[..] else { panic!("{:?}", ids(&s)) };
    s.execute("mask.move", &json!({"id": c, "to": 0})).unwrap();
    assert_eq!(ids(&s), [c, a, b]);
    s.execute("mask.move", &json!({"id": c, "delta": 1})).unwrap();
    assert_eq!(ids(&s), [a, c, b]);
    s.execute("mask.move", &json!({"id": a, "delta": -5})).unwrap();
    assert_eq!(ids(&s), [a, c, b], "clamped at the top");
    assert!(s.execute("mask.move", &json!({"id": a})).is_err());
    // one undo step per move
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(ids(&s), [c, a, b]);
    // duplicate and invert: right after the original, inverted, selected
    let r = s.execute("mask.duplicate", &json!({"id": a, "invert": true})).unwrap();
    let copy = r["activeMask"].as_u64().unwrap() as u32;
    let d = active_dev(&s);
    let pos = d.masks.iter().position(|m| m.id == copy).unwrap();
    assert_eq!(d.masks[pos - 1].id, a);
    assert!(d.masks[pos].invert && !d.masks[pos - 1].invert);
    assert_eq!(d.masks[pos].components, d.masks[pos - 1].components);
    assert_eq!(s.active_mask, Some(copy));
}

#[test]
fn before_is_the_import_state_and_can_be_set_copied_and_swapped() {
    let mut s = demo();
    let id = s.active().unwrap();
    let import = s.catalog.photo(id).unwrap().import_defaults();
    s.execute("develop.set", &json!({"values": {"light.exposure": 1.5}})).unwrap();
    s.execute("crop.set", &json!({"rect": [0.1, 0.1, 0.9, 0.9]})).unwrap();
    let before = |s: &Session| s.before_settings(s.catalog.photo(s.active().unwrap()).unwrap());
    // default: the import state, with the current crop so both sides line up
    let b = before(&s);
    assert_eq!(b.light.exposure, import.light.exposure);
    assert_eq!(b.crop, active_dev(&s).crop);
    // from the current settings, then a history step
    s.execute("beforeAfter.copyAfterToBefore", &json!({})).unwrap();
    assert_eq!(before(&s).light.exposure, 1.5);
    s.execute("develop.set", &json!({"values": {"light.exposure": -1.0}})).unwrap();
    let steps = s.catalog.photo(id).unwrap().history.len();
    let r = s.execute("beforeAfter.setBefore", &json!({"source": "history", "index": steps - 1})).unwrap();
    assert_eq!(r["before"], "custom");
    assert_eq!(before(&s).light.exposure, -1.0);
    assert!(s.execute("beforeAfter.setBefore", &json!({"source": "history", "index": 999})).is_err());
    // swap: the photo gets the before settings, the before side the old current ones
    s.execute("beforeAfter.setBefore", &json!({"source": "import"})).unwrap();
    s.execute("beforeAfter.swap", &json!({})).unwrap();
    assert_eq!(active_dev(&s).light.exposure, import.light.exposure);
    assert_eq!(before(&s).light.exposure, -1.0);
    // copy before → after is one undoable edit
    s.execute("beforeAfter.copyBeforeToAfter", &json!({})).unwrap();
    assert_eq!(active_dev(&s).light.exposure, -1.0);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(active_dev(&s).light.exposure, import.light.exposure);
    // the before render follows (the render key changes with the before settings)
    let k1 = s.render_job(id, 64, 64, true, true).unwrap().key;
    s.execute("beforeAfter.resetBefore", &json!({})).unwrap();
    let k2 = s.render_job(id, 64, 64, true, true).unwrap().key;
    assert_ne!(k1, k2);
}

#[test]
fn presets_versions_history_and_select_by() {
    let mut s = demo();
    s.execute("develop.set", &json!({"values": {"light.exposure": 0.5}})).unwrap();
    let pid = s.execute("preset.create", &json!({"name": "Warm", "groups": ["light"]})).unwrap()["id"].as_str().unwrap().to_string();
    let preset = |s: &Session| s.presets.iter().find(|p| p.id == pid).cloned().unwrap();
    s.execute("preset.rename", &json!({"id": pid, "name": "Warmer"})).unwrap();
    s.execute("preset.move", &json!({"id": pid, "group": "Mine"})).unwrap();
    assert_eq!((preset(&s).name.as_str(), preset(&s).group.as_str()), ("Warmer", "Mine"));
    // update keeps the preset's groups and takes the current values
    s.execute("develop.set", &json!({"values": {"light.exposure": 1.25, "color.vibrance": 40}})).unwrap();
    s.execute("preset.update", &json!({"id": pid})).unwrap();
    assert_eq!(preset(&s).settings["light"]["exposure"], 1.25);
    assert!(preset(&s).settings.get("color").is_none(), "only the groups it already had");
    let builtin = s.presets.iter().find(|p| p.builtin).unwrap().id.clone();
    assert!(s.execute("preset.rename", &json!({"id": builtin, "name": "x"})).is_err());
    // versions
    s.execute("version.create", &json!({"name": "A"})).unwrap();
    s.execute("version.rename", &json!({"index": 0, "name": "First"})).unwrap();
    s.execute("develop.set", &json!({"values": {"light.exposure": -2.0}})).unwrap();
    s.execute("version.update", &json!({"index": 0})).unwrap();
    let v = s.catalog.photo(s.active().unwrap()).unwrap().versions[0].clone();
    assert_eq!((v.name.as_str(), v.settings.light.exposure), ("First", -2.0));
    // history
    s.execute("history.clear", &json!({})).unwrap();
    let h = &s.catalog.photo(s.active().unwrap()).unwrap().history;
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].settings.light.exposure, -2.0);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.catalog.photo(s.active().unwrap()).unwrap().history.len() > 1, "undoable");
    // select by: the demo has rated and picked photos
    let vis = s.visible_cloned();
    let picks = vis.iter().filter(|id| s.catalog.photo(**id).unwrap().flag == lightcraft_catalog::Flag::Pick).count();
    let r = s.execute("library.selectBy", &json!({"flag": "pick"})).unwrap();
    assert_eq!(r["selected"].as_u64(), Some(picks as u64));
    assert!(picks > 0);
    let four = vis.iter().filter(|id| s.catalog.photo(**id).unwrap().rating >= 4).count();
    assert_eq!(s.execute("library.selectBy", &json!({"rating": 4})).unwrap()["selected"].as_u64(), Some(four as u64));
    let r = s.execute("library.selectBy", &json!({"rating": 0, "ratingOp": "eq", "add": true})).unwrap();
    assert!(r["selected"].as_u64().unwrap() >= four as u64);
    assert!(s.execute("library.selectBy", &json!({})).is_err());
    assert!(s.execute("library.selectBy", &json!({"label": "mauve"})).is_err());
}

#[test]
fn crop_aspect_lock_current_and_toggle() {
    let mut s = demo();
    s.execute("crop.set", &json!({"rect": [0.1, 0.2, 0.7, 0.6]})).unwrap();
    let rect = active_dev(&s).crop.geometry.rect;
    s.execute("crop.aspect", &json!({"aspect": "toggle"})).unwrap();
    let d = active_dev(&s);
    assert_eq!(d.crop.geometry.rect, rect, "locking keeps the rectangle");
    let (aw, ah) = d.crop.aspect.expect("locked");
    let id = s.active().unwrap();
    let p = s.catalog.photo(id).unwrap();
    let (w, h) = if d.orientation.swaps_axes() { (p.height as f64, p.width as f64) } else { (p.width as f64, p.height as f64) };
    let want = (rect.x1 - rect.x0) * w / ((rect.y1 - rect.y0) * h);
    assert!((aw as f64 / ah as f64 - want).abs() < 0.01, "{aw}:{ah} vs {want}");
    s.execute("crop.aspect", &json!({"aspect": "toggle"})).unwrap();
    assert!(active_dev(&s).crop.aspect.is_none(), "toggle unlocks");
}

#[test]
fn paste_from_previous_and_copy_paste_metadata() {
    let mut s = demo();
    let ids: Vec<_> = s.visible().iter().copied().take(3).collect();
    s.execute("library.select", &json!({"ids": [ids[0].0]})).unwrap();
    assert!(s.execute("develop.pastePrevious", &json!({})).is_err(), "nothing before");
    s.execute("develop.set", &json!({"values": {"light.exposure": 0.8}})).unwrap();
    s.execute("photo.setMeta", &json!({"title": "Dawn", "keywords": ["sky", "red"], "creator": "Me"})).unwrap();
    s.execute("photo.copyMetadata", &json!({})).unwrap();
    // move on: the first photo becomes "previous"
    s.execute("library.select", &json!({"ids": [ids[1].0]})).unwrap();
    assert_eq!(s.previous_active, Some(ids[0]));
    let r = s.execute("develop.pastePrevious", &json!({})).unwrap();
    assert_eq!(r["from"].as_u64(), Some(ids[0].0));
    assert_eq!(active_dev(&s).light.exposure, 0.8);
    // metadata: only the chosen fields
    s.execute("photo.pasteMetadata", &json!({"fields": ["title", "keywords"]})).unwrap();
    let m = &s.catalog.photo(ids[1]).unwrap().meta;
    assert_eq!((m.title.as_str(), m.keywords.clone()), ("Dawn", vec!["sky".to_string(), "red".to_string()]));
    assert_ne!(m.creator, "Me");
    s.execute("photo.pasteMetadata", &json!({"ids": [ids[2].0]})).unwrap();
    assert_eq!(s.catalog.photo(ids[2]).unwrap().meta.creator, "Me");
}

#[test]
fn stacks_move_and_split() {
    let mut s = demo();
    let ids: Vec<_> = s.visible().iter().copied().take(5).collect();
    s.execute("library.select", &json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>(), "active": ids[0].0})).unwrap();
    s.execute("stack.group", &json!({"collapsed": false})).unwrap();
    let order = |s: &Session| s.catalog.stack_of(ids[0]).or(s.catalog.stack_of(ids[4])).map(|st| st.photos.clone()).unwrap_or_default();
    let before = order(&s);
    assert_eq!(before.len(), 5);
    s.execute("stack.moveUp", &json!({"id": before[2].0})).unwrap();
    assert_eq!(order(&s)[1], before[2]);
    s.execute("stack.moveDown", &json!({"id": before[2].0})).unwrap();
    assert_eq!(order(&s), before);
    // split at the 4th photo: 3 stay, 2 form a new stack
    s.execute("stack.split", &json!({"id": before[3].0})).unwrap();
    assert_eq!(s.catalog.stack_of(before[0]).unwrap().photos, before[..3].to_vec());
    assert_eq!(s.catalog.stack_of(before[3]).unwrap().photos, before[3..].to_vec());
    // splitting at the last photo of a 2-stack leaves both unstacked
    s.execute("stack.split", &json!({"id": before[4].0})).unwrap();
    assert!(s.catalog.stack_of(before[3]).is_none() && s.catalog.stack_of(before[4]).is_none());
    assert!(s.execute("stack.split", &json!({"id": before[0].0})).is_err(), "the top can't split");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.catalog.stack_of(before[3]).unwrap().photos.len(), 2);
}

#[test]
fn filter_presets_save_and_apply() {
    let mut s = demo();
    assert!(s.execute("filter.savePreset", &json!({"name": "Empty"})).is_err(), "nothing to save");
    s.execute("library.filter", &json!({"rating": 4})).unwrap();
    let four = s.visible_cloned().len();
    s.execute("filter.savePreset", &json!({"name": "Best"})).unwrap();
    s.execute("library.clearFilter", &json!({})).unwrap();
    assert!(s.visible_cloned().len() > four);
    let r = s.execute("filter.applyPreset", &json!({"name": "best"})).unwrap();
    assert_eq!(r["photos"].as_u64(), Some(four as u64));
    s.execute("filter.deletePreset", &json!({"name": "Best"})).unwrap();
    assert!(s.filter_presets.is_empty());
}

#[test]
fn auto_bw_mix_separates_colours() {
    let mut s = demo();
    s.execute("develop.autoBwMix", &json!({})).unwrap();
    let d = active_dev(&s);
    assert_eq!(d.treatment, lightcraft_develop::Treatment::Bw);
    let m = d.bw_mix.bands();
    assert!(m.iter().any(|v| *v != 0.0), "some band moved: {m:?}");
    assert!(m.iter().all(|v| v.abs() <= 60.0));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_ne!(active_dev(&s).treatment, lightcraft_develop::Treatment::Bw, "one undo step");
}

#[test]
fn object_masks_check_their_prompt() {
    use lightcraft_develop::MaskShape;
    let mut s = demo();
    let object = |s: &Session, i: usize| match &active_dev(s).masks[i].components[0].shape {
        MaskShape::Object { hint, bbox, exclude } => (hint.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(), *bbox, exclude.len()),
        other => panic!("not an object mask: {other:?}"),
    };
    // a click; a box drawn right to left (normalized); points a hair off the edge (clamped)
    s.execute("mask.add", &json!({"kind": "object", "points": [[0.4, 0.5]]})).unwrap();
    assert_eq!(object(&s, 0), (vec![(0.4, 0.5)], None, 0));
    s.execute("mask.add", &json!({"kind": "object", "box": [0.7, 0.8, 0.2, 0.1]})).unwrap();
    assert_eq!(object(&s, 1), (vec![], Some([0.2, 0.1, 0.7, 0.8]), 0));
    s.execute("mask.add", &json!({"kind": "object", "points": [[1.005, -0.004]], "exclude": [[0.1, 0.1]]})).unwrap();
    assert_eq!(object(&s, 2), (vec![(1.0, 0.0)], None, 1));
    // bad requests are refused and change nothing
    let bad = [
        json!({"kind": "object"}),
        json!({"kind": "object", "points": []}),
        json!({"kind": "object", "points": [[1.5, 0.5]]}),
        json!({"kind": "object", "points": [[0.5]]}),
        json!({"kind": "object", "points": "middle"}),
        json!({"kind": "object", "points": [["a", "b"]]}),
        json!({"kind": "object", "exclude": [[0.5, 0.5]]}),
        json!({"kind": "object", "box": [0.1, 0.2, 0.3]}),
        json!({"kind": "object", "box": [0.1, 0.2, 0.3, 0.4, 0.5]}),
        json!({"kind": "object", "box": [0.1, 0.1, 0.1005, 0.5]}),
        json!({"kind": "object", "box": [-0.5, 0.1, 0.5, 0.5]}),
        json!({"kind": "object", "box": "all"}),
    ];
    for b in &bad {
        let e = s.execute("mask.add", b).expect_err(&b.to_string());
        assert!(e.to_string().contains("mask.add"), "{e}");
    }
    assert_eq!(active_dev(&s).masks.len(), 3, "nothing was added by the bad requests");
    // refining (Shift/Alt-click in the app) goes through the same checks
    let mid = active_dev(&s).masks[0].id;
    let good =
        json!({"id": mid, "shape": {"kind": "object", "hint": [{"x": 0.4, "y": 0.5}, {"x": 0.45, "y": 0.55}], "exclude": [{"x": 0.9, "y": 0.9}]}});
    s.execute("mask.update", &good).unwrap();
    assert_eq!(object(&s, 0), (vec![(0.4, 0.5), (0.45, 0.55)], None, 1));
    let off = json!({"id": mid, "shape": {"kind": "object", "hint": [{"x": 3.0, "y": 0.5}]}});
    assert!(s.execute("mask.update", &off).is_err(), "a point off the photo");
    let empty = json!({"id": mid, "shape": {"kind": "object", "hint": []}});
    assert!(s.execute("mask.update", &empty).is_err(), "an empty prompt");
    assert_eq!(object(&s, 0).0.len(), 2, "the refused updates changed nothing");
    // no models in this session: the command says so
    let m = s.execute("mask.models", &json!({})).unwrap();
    assert_eq!(m, json!({"dir": null, "sky": false, "subject": false, "object": false, "busy": false}));
    // without the models the photo still renders (the masks select nothing yet)
    let id = s.active().unwrap();
    let r = s.render_now(id, 120, 80).unwrap();
    let (w, h) = (r.image.width, r.image.height);
    assert!(w <= 120 && h <= 80 && (w == 120 || h == 80), "fits the box keeping its shape: {w}×{h}");
}
