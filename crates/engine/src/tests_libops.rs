//! Library management commands: date groups, keyword rename/delete/merge, batch rename, capture
//! time, label names, import review.

use serde_json::json;

use crate::Session;

/// Date headers follow the grid order and the sort's grouping.
#[test]
fn date_groups_follow_sort_and_grouping() {
    let mut s = Session::with_demo();
    let vis = s.visible_cloned();
    let g = s.execute("library.groups", &json!({})).unwrap();
    let groups = g.as_array().unwrap();
    assert!(groups.len() > 3, "{g}");
    let total: u64 = groups.iter().map(|x| x["count"].as_u64().unwrap()).sum();
    assert_eq!(total as usize, vis.len());
    // contiguous runs in grid order, labelled with weekday and date
    let mut next = 0;
    for x in groups {
        assert_eq!(x["start"].as_u64().unwrap(), next);
        next += x["count"].as_u64().unwrap();
        assert!(x["label"].as_str().unwrap().contains(", "), "{x}");
    }
    let months = s.execute("library.groups", &json!({"by": "month"})).unwrap();
    assert!(months.as_array().unwrap().len() < groups.len());
    assert_eq!(months[0]["key"].as_str().unwrap().len(), 7);
    s.execute("library.sort", &json!({"group": "none"})).unwrap();
    assert_eq!(s.execute("library.groups", &json!({})).unwrap(), json!([]));
    s.execute("library.sort", &json!({"group": "year", "key": "fileName"})).unwrap();
    assert_eq!(s.execute("library.groups", &json!({})).unwrap(), json!([]), "no date headers when sorting by name");
    assert_eq!(s.sort.group, lightcraft_catalog::GroupBy::Year);
    assert!(s.execute("library.sort", &json!({"group": "week"})).is_err());
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-libops-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn keywords_of(s: &Session, id: u64) -> Vec<String> {
    s.catalog.photo(lightcraft_catalog::PhotoId(id)).unwrap().meta.keywords.clone()
}

/// Keyword rename / merge / delete: one undo step each, replayed from the op log after a restart,
/// and the keyword filter follows the renamed keyword.
#[test]
fn keyword_rename_merge_delete_undo_and_replay() {
    let dir = temp_dir("kw");
    let mut s = Session::new();
    s.open_library(&dir, true).unwrap();
    let ids: Vec<u64> = s.visible_cloned().iter().take(4).map(|p| p.0).collect();
    s.execute("photo.setMeta", &json!({"ids": [ids[0], ids[1]], "keywords": ["travel|italy|rome", "sea"]})).unwrap();
    s.execute("photo.setMeta", &json!({"ids": [ids[2]], "keywords": ["Italia"]})).unwrap();
    s.execute("photo.setMeta", &json!({"ids": [ids[3]], "keywords": ["travel|france", "sea"]})).unwrap();
    let tree = s.execute("keyword.list", &json!({})).unwrap();
    let travel = tree.as_array().unwrap().iter().find(|n| n["name"] == "travel").unwrap().clone();
    assert_eq!(travel["count"], 3, "{tree}");
    assert_eq!(travel["children"][1]["path"], "travel|italy");
    // filter by a parent keyword: its children match
    s.execute("library.filter", &json!({"keyword": "travel|italy"})).unwrap();
    assert_eq!(s.visible().len(), 2);
    let undo_before = s.undo.len();
    let r = s.execute("keyword.rename", &json!({"from": "travel|italy", "to": "Europe|Italy"})).unwrap();
    assert_eq!(r["changed"], 2);
    assert_eq!(s.undo.len(), undo_before + 1, "one undo step");
    assert_eq!(keywords_of(&s, ids[0]), ["Europe|Italy|rome", "sea"]);
    assert_eq!(s.filter.keyword.as_deref(), Some("Europe|Italy"), "the filter follows the rename");
    assert_eq!(s.visible().len(), 2);
    s.execute("keyword.merge", &json!({"from": ["Italia"], "into": "Europe|Italy"})).unwrap();
    assert_eq!(keywords_of(&s, ids[2]), ["Europe|Italy"]);
    assert_eq!(s.visible().len(), 3);
    s.execute("keyword.delete", &json!({"keyword": "travel"})).unwrap();
    assert_eq!(keywords_of(&s, ids[3]), ["sea"]);
    // suggestions: keywords used together with `sea` first
    let sug = s.execute("keyword.suggest", &json!({"ids": [ids[3]]})).unwrap();
    assert_eq!(sug[0], "Europe|Italy|rome", "{sug}");
    let sug = s.execute("keyword.suggest", &json!({"ids": [ids[3]], "prefix": "eur"})).unwrap();
    assert_eq!(sug, json!(["Europe|Italy|rome", "Europe|Italy"]), "most used first");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(keywords_of(&s, ids[3]), ["travel|france", "sea"]);
    s.execute("edit.redo", &json!({})).unwrap();
    // restart without a clean close: the log replays to the same state
    let expect = s.catalog.to_snapshot();
    drop(s);
    let mut s = Session::new();
    s.open_library(&dir, false).unwrap();
    assert_eq!(s.catalog.to_snapshot(), expect);
    // invalid
    assert!(s.execute("keyword.rename", &json!({"from": "sea", "to": ""})).is_err());
    assert!(s.execute("keyword.merge", &json!({"from": [], "into": "x"})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

fn write_png(path: &std::path::Path, seed: u8) {
    let (w, h) = (24usize, 16usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 9) as u8, (i / w * 11) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn path_of(s: &Session, id: u64) -> String {
    match &s.catalog.photo(lightcraft_catalog::PhotoId(id)).unwrap().source {
        lightcraft_catalog::Source::File { path } => path.clone(),
        _ => panic!("not a file"),
    }
}

/// Batch rename on disk: collisions with existing files and within the batch get suffixes, sidecars
/// move along, virtual copies follow, undo/redo move the files back and forth, a failed move rolls
/// the batch back, and the op log replays the new paths.
#[test]
fn batch_rename_files_collisions_undo_and_replay() {
    let src = temp_dir("rename-src");
    let lib = temp_dir("rename-lib");
    write_png(&src.join("a.png"), 1);
    write_png(&src.join("b.png"), 2);
    std::fs::write(src.join("Trip-001.png"), b"not ours").unwrap();
    std::fs::write(src.join("a.xmp"), b"<x:xmpmeta xmlns:x='adobe:ns:meta/'></x:xmpmeta>").unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [src.join("a.png").to_string_lossy(), src.join("b.png").to_string_lossy()]})).unwrap();
    let ids: Vec<u64> = r["imported"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert_eq!(ids.len(), 2, "{r}");
    s.execute("library.select", &json!({"ids": [ids[0]]})).unwrap();
    s.execute("photo.virtualCopy", &json!({})).unwrap();
    let copy = s.catalog.photos().find(|p| p.copy_of.is_some()).unwrap().id.0;

    // preview: the existing Trip-001.png is skipped, the copy shares its master's file
    let pv = s.execute("photo.renamePreview", &json!({"ids": [ids[0], copy, ids[1]], "template": "Trip-{seq:3}"})).unwrap();
    assert_eq!(pv.as_array().unwrap().len(), 2, "{pv}");
    assert_eq!(pv[0]["to"], "Trip-001-1.png");
    assert_eq!(pv[1]["to"], "Trip-002.png");
    let r = s.execute("photo.rename", &json!({"ids": [ids[0], copy, ids[1]], "template": "Trip-{seq:3}"})).unwrap();
    assert_eq!(r["renamed"], 3, "{r}");
    assert!(src.join("Trip-001-1.png").is_file() && src.join("Trip-002.png").is_file());
    assert!(!src.join("a.png").exists() && !src.join("b.png").exists());
    assert_eq!(std::fs::read(src.join("Trip-001.png")).unwrap(), b"not ours", "never overwritten");
    assert!(src.join("Trip-001-1.xmp").is_file() && !src.join("a.xmp").exists(), "sidecar moved");
    assert_eq!(path_of(&s, copy), path_of(&s, ids[0]), "the virtual copy follows");
    assert_eq!(s.catalog.photo(lightcraft_catalog::PhotoId(ids[1])).unwrap().file_name, "Trip-002.png");

    // undo moves the files back; redo renames again
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(src.join("a.png").is_file() && src.join("b.png").is_file() && src.join("a.xmp").is_file());
    assert_eq!(path_of(&s, ids[0]), src.join("a.png").to_string_lossy());
    s.execute("edit.redo", &json!({})).unwrap();
    assert!(src.join("Trip-002.png").is_file() && !src.join("b.png").exists());

    // collisions inside the batch
    let r = s.execute("photo.rename", &json!({"ids": [ids[0], ids[1]], "template": "same"})).unwrap();
    assert_eq!(r["plans"][0]["to"], "same.png");
    assert_eq!(r["plans"][1]["to"], "same-1.png");

    // a failing move rolls the whole batch back and changes nothing
    std::fs::remove_file(src.join("same-1.png")).unwrap();
    let before = s.catalog.to_snapshot();
    assert!(s.execute("photo.rename", &json!({"ids": [ids[0], ids[1]], "template": "x-{seq}"})).is_err());
    assert!(src.join("same.png").is_file() && !src.join("x-1.png").exists(), "rolled back");
    assert_eq!(s.catalog.to_snapshot(), before);

    // the op log replays the renames
    let expect = s.catalog.to_snapshot();
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    assert_eq!(s.catalog.to_snapshot(), expect);
    assert_eq!(path_of(&s, ids[0]), src.join("same.png").to_string_lossy());
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

/// Demo photos (no file) rename in the catalog only.
#[test]
fn rename_demo_photos_in_catalog() {
    let mut s = Session::with_demo();
    let ids: Vec<u64> = s.visible_cloned().iter().take(3).map(|p| p.0).collect();
    let r = s.execute("photo.rename", &json!({"ids": ids, "template": "{date:%Y-%m-%d}_{seq:2}", "start": 7})).unwrap();
    assert_eq!(r["renamed"], 3);
    let name = &s.catalog.photo(lightcraft_catalog::PhotoId(ids[0])).unwrap().file_name;
    assert!(name.ends_with("_07.jpg") || name.contains("_07."), "{name}");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.catalog.photo(lightcraft_catalog::PhotoId(ids[0])).unwrap().file_name.starts_with("LC"));
}

fn captured(s: &Session, id: u64) -> Option<String> {
    s.catalog.photo(lightcraft_catalog::PhotoId(id)).unwrap().captured.clone()
}

/// Edit Capture Time: set (others shift along), set each, shift by an offset, time-zone shift;
/// one undo step each; replayed from the op log; the grid order follows.
#[test]
fn capture_time_set_shift_undo_and_replay() {
    let dir = temp_dir("capture");
    let mut s = Session::new();
    s.open_library(&dir, true).unwrap();
    let ids: Vec<u64> = s.visible_cloned().iter().take(3).map(|p| p.0).collect();
    let before: Vec<Option<String>> = ids.iter().map(|i| captured(&s, *i)).collect();
    let secs = |t: &Option<String>| lightcraft_catalog::dates::iso_seconds(t.as_deref().unwrap()).unwrap();
    // set the active photo; the others keep their distance to it
    s.execute("library.select", &json!({"ids": ids, "active": ids[1]})).unwrap();
    let r = s.execute("photo.setCaptureTime", &json!({"time": "2020-01-02 03:04:05"})).unwrap();
    assert_eq!(r["changed"], 3, "{r}");
    assert_eq!(captured(&s, ids[1]).as_deref(), Some("2020-01-02T03:04:05"));
    assert_eq!(secs(&captured(&s, ids[0])) - secs(&captured(&s, ids[1])), secs(&before[0]) - secs(&before[1]));
    // undo restores all three in one step
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(ids.iter().map(|i| captured(&s, *i)).collect::<Vec<_>>(), before);
    // shift by +1 h 30 min, then a time-zone shift of −2 h
    s.execute("photo.setCaptureTime", &json!({"shift": 5400})).unwrap();
    s.execute("photo.setCaptureTime", &json!({"hours": -2})).unwrap();
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(secs(&captured(&s, *id)) - secs(&before[i]), 5400 - 7200);
    }
    // everybody the same time
    s.execute("photo.setCaptureTime", &json!({"time": "2026-10-01T08:00:00", "each": true})).unwrap();
    assert!(ids.iter().all(|i| captured(&s, *i).as_deref() == Some("2026-10-01T08:00:00")));
    // the grid order (capture date, newest first) follows: they're now the newest photos
    let vis = s.visible_cloned();
    assert!(vis[..3].iter().all(|v| ids.contains(&v.0)), "{vis:?}");
    // invalid dates are refused
    assert!(s.execute("photo.setCaptureTime", &json!({"time": "2026-02-30 10:00"})).is_err());
    assert!(s.execute("photo.setCaptureTime", &json!({"time": "yesterday"})).is_err());
    let expect = s.catalog.to_snapshot();
    drop(s);
    let mut s = Session::new();
    s.open_library(&dir, false).unwrap();
    assert_eq!(s.catalog.to_snapshot(), expect);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Colour label names: one undo step, journaled; a colour's own name or empty resets it.
#[test]
fn label_names_set_undo_and_replay() {
    let dir = temp_dir("labels");
    let mut s = Session::new();
    s.open_library(&dir, true).unwrap();
    let r = s.execute("label.setNames", &json!({"names": {"red": "Reject later", "green": "Approved", "blue": "  "}})).unwrap();
    assert_eq!(r["changed"], 2);
    let names = s.execute("label.names", &json!({})).unwrap();
    assert_eq!(names[0], json!({"label": "red", "name": "Reject later", "custom": "Reject later"}));
    assert_eq!(names[3]["name"], "Blue");
    assert_eq!(names[3]["custom"], json!(null));
    s.execute("label.setNames", &json!({"names": {"red": "red"}})).unwrap();
    assert_eq!(s.catalog.label_name(lightcraft_catalog::ColorLabel::Red), "Red", "the colour's own name resets");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.catalog.label_name(lightcraft_catalog::ColorLabel::Red), "Reject later");
    assert!(s.execute("label.setNames", &json!({"names": {"orange": "x"}})).is_err());
    let expect = s.catalog.to_snapshot();
    drop(s);
    let mut s = Session::new();
    s.open_library(&dir, false).unwrap();
    assert_eq!(s.catalog.to_snapshot(), expect);
    assert_eq!(s.catalog.label_name(lightcraft_catalog::ColorLabel::Green), "Approved");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Import review: `library.importPreview` lists candidates with duplicates marked (nothing is
/// added); `library.import` takes the checked files with preset, keywords and a new album as one
/// undo step, and copies into the dated library folders.
#[test]
fn import_review_preview_and_options() {
    let src = temp_dir("imp-src");
    let lib = temp_dir("imp-lib");
    write_png(&src.join("a.png"), 1);
    write_png(&src.join("sub/b.png"), 2);
    write_png(&src.join("sub/c.png"), 3);
    std::fs::copy(src.join("a.png"), src.join("sub/a-again.png")).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    // c.png is already in the library (spelled with native separators, as the folder scan finds it)
    s.execute("library.import", &json!({"paths": [src.join("sub").join("c.png").to_string_lossy()]})).unwrap();
    let n0 = s.catalog.len();
    let undo0 = s.undo.len();
    let pv = s.execute("library.importPreview", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(pv["scanned"], 4, "{pv}");
    assert_eq!(pv["duplicates"], 2, "{pv}");
    assert_eq!(s.catalog.len(), n0, "a preview adds nothing");
    assert_eq!(s.undo.len(), undo0);
    let cands = pv["candidates"].as_array().unwrap();
    let dup = |name: &str| cands.iter().find(|c| c["name"] == name).unwrap()["duplicate"].clone();
    assert_eq!(dup("c.png"), "path");
    assert_eq!(dup("a-again.png"), "content");
    assert_eq!(dup("a.png"), json!(null));
    assert_eq!(cands[0]["width"], 24);
    // the dialog imports the checked (non-duplicate) files with its options
    let preset = s.presets.iter().find(|p| p.builtin).unwrap().id.clone();
    let checked: Vec<String> = cands.iter().filter(|c| c["duplicate"].is_null()).map(|c| c["path"].as_str().unwrap().to_string()).collect();
    let r = s
        .execute(
            "library.import",
            &json!({"paths": checked, "mode": "copy", "preset": preset, "keywords": ["trip", "family|kids"], "albumName": "Imported Trip"}),
        )
        .unwrap();
    assert_eq!(r["imported"].as_array().unwrap().len(), 2, "{r}");
    let album = lightcraft_catalog::AlbumId(r["album"].as_u64().unwrap());
    assert_eq!(s.catalog.album(album).unwrap().name, "Imported Trip");
    assert_eq!(s.catalog.album_count(album), 2);
    for id in r["imported"].as_array().unwrap() {
        let p = s.catalog.photo(lightcraft_catalog::PhotoId(id.as_u64().unwrap())).unwrap();
        assert_eq!(p.meta.keywords, ["trip", "family|kids"]);
        assert!(p.history.last().unwrap().label.starts_with("Preset: "), "preset applied with a History entry");
        let lightcraft_catalog::Source::File { path } = &p.source else { panic!() };
        assert!(std::path::Path::new(path).starts_with(lib.join("Originals")), "{path}");
    }
    assert_eq!(s.undo.len(), undo0 + 1, "import + album = one undo step");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.catalog.len(), n0);
    assert!(s.catalog.album(album).is_none());
    assert!(s.execute("library.import", &json!({"paths": ["/nope"], "preset": "no-such"})).is_err());
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}
