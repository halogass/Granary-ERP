//! Golden fixtures recorded from backend/app/services/wa_export.py (compat-tests/golden/gen_wa_export.py).
//!
//! Everything is compared for exact equality except the re-encoded images: Pillow and the `image` crate
//! resize and JPEG-encode differently, so those are checked for the same size and colour type and for a
//! small mean per-channel difference after decoding (the measured values are printed).

use base64::Engine as _;
use chrono_tz::Tz;
use granary_services::wa_export::{
    _shrink_image, build_pdf_messages, build_txt, filter_range, media_label, parse_day,
    thumbnail_size, MAX_IMAGE_EDGE,
};
use serde_json::{json, Value};

#[path = "golden/bless.rs"]
mod bless;

fn load(name: &str) -> Value {
    let path = format!(
        "{}/../../compat-tests/golden/wa_export/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json")
}

fn tz(v: &Value) -> Tz {
    v.as_str().unwrap().parse().unwrap()
}

fn arr(v: &Value) -> Vec<Value> {
    v.as_array().unwrap().clone()
}

#[test]
fn golden_parse_day() {
    let mut cases = load("parse_day");
    let mut n = 0;
    for c in cases.as_array_mut().unwrap() {
        let got = parse_day(c["in"].as_str(), tz(&c["tz"]), c["end"].as_bool().unwrap());
        if bless::blessing() {
            c["out"] = got.as_ref().map_or(
                Value::Null,
                |l| json!({"iso": l.isoformat(), "ts": l.utc.timestamp() as f64}),
            );
        }
        match (&got, c["out"].is_null()) {
            (None, true) => {}
            (Some(l), false) => {
                assert_eq!(l.isoformat(), c["out"]["iso"].as_str().unwrap(), "{c}");
                assert_eq!(
                    l.utc.timestamp() as f64,
                    c["out"]["ts"].as_f64().unwrap(),
                    "{c}"
                );
                n += 1;
            }
            _ => panic!("{c} -> {:?}", got.map(|l| l.py.isoformat())),
        }
    }
    bless::save("wa_export/parse_day.json", &cases);
    assert!(n > 200, "{n}");
}

#[test]
fn golden_filter_range() {
    let mut f = load("filter_range");
    let msgs = arr(&f["messages"]);
    for c in f["cases"].as_array_mut().unwrap() {
        let got: Vec<Value> =
            filter_range(&msgs, c["from"].as_str(), c["to"].as_str(), tz(&c["tz"]))
                .into_iter()
                .map(|m| m["id"].clone())
                .collect();
        let msg = c.to_string();
        bless::eq(&mut c["ids"], Value::Array(got), msg);
    }
    bless::save("wa_export/filter_range.json", &f);
}

#[test]
fn golden_media_label() {
    for c in load("media_label").as_array().unwrap() {
        assert_eq!(media_label(&c["in"]), c["out"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn golden_build_txt() {
    let mut cases = load("build_txt");
    for c in cases.as_array_mut().unwrap() {
        let meta: Vec<String> = arr(&c["meta"])
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let got = build_txt(
            c["title"].as_str().unwrap(),
            c["subtitle"].as_str().unwrap(),
            &meta,
            &arr(&c["messages"]),
            c["me"].as_str().unwrap(),
            tz(&c["tz"]),
        );
        if bless::blessing() {
            c["out"] = got.clone().into();
        }
        let want = c["out"].as_str().unwrap();
        if got != want {
            for (a, b) in got.lines().zip(want.lines()) {
                assert_eq!(a, b);
            }
        }
        assert_eq!(got, want);
    }
    bless::save("wa_export/build_txt.json", &cases);
}

#[test]
fn golden_build_pdf_messages() {
    let mut cases = load("build_pdf_messages");
    for c in cases.as_array_mut().unwrap() {
        let got = build_pdf_messages(
            &arr(&c["messages"]),
            c["me"].as_str().unwrap(),
            c["them"].as_str().unwrap(),
            tz(&c["tz"]),
        )
        .unwrap();
        if bless::blessing() {
            c["out"] = Value::Array(got.clone());
        }
        let want = arr(&c["out"]);
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(want.iter()) {
            assert_eq!(g, w);
            // Same key order as the Python dict.
            let gk: Vec<&String> = g.as_object().unwrap().keys().collect();
            let wk: Vec<&String> = w.as_object().unwrap().keys().collect();
            assert_eq!(gk, wk);
        }
    }
    bless::save("wa_export/build_pdf_messages.json", &cases);
    for c in load("build_pdf_errors").as_array().unwrap() {
        let err = build_pdf_messages(&arr(&c["messages"]), "a", "b", chrono_tz::UTC).unwrap_err();
        assert_eq!(err, c["error"].as_str().unwrap());
    }
}

#[test]
fn golden_thumbnail_size() {
    for c in load("thumbnail").as_array().unwrap() {
        let (w, h) = (
            c["w"].as_u64().unwrap() as u32,
            c["h"].as_u64().unwrap() as u32,
        );
        let got = thumbnail_size(w, h, MAX_IMAGE_EDGE).unwrap_or((w, h));
        let want = (
            c["out"][0].as_u64().unwrap() as u32,
            c["out"][1].as_u64().unwrap() as u32,
        );
        assert_eq!(got, want, "{w}x{h}");
    }
}

fn decode_data_url(url: &str) -> image::DynamicImage {
    let (prefix, b64) = url.split_once(',').unwrap();
    assert_eq!(prefix, "data:image/jpeg;base64");
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    assert_eq!(image::guess_format(&raw).unwrap(), image::ImageFormat::Jpeg);
    image::load_from_memory(&raw).unwrap()
}

#[test]
fn golden_shrink_image() {
    for c in load("shrink_image").as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let got = _shrink_image(&c["data"], "image/x");
        if c["out_b64"].is_null() {
            assert!(got.is_none(), "{name}");
            continue;
        }
        let got = decode_data_url(&got.expect(name));
        let want = image::load_from_memory(
            &base64::engine::general_purpose::STANDARD
                .decode(c["out_b64"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        let size = (
            c["size"][0].as_u64().unwrap() as u32,
            c["size"][1].as_u64().unwrap() as u32,
        );
        assert_eq!((got.width(), got.height()), size, "{name}");
        let gray = c["mode"].as_str().unwrap() == "L";
        assert_eq!(
            matches!(got, image::DynamicImage::ImageLuma8(_)),
            gray,
            "{name}: colour type"
        );
        // Before JPEG: the resized pixels against Pillow's thumbnail.
        let pre = image::load_from_memory(
            &base64::engine::general_purpose::STANDARD
                .decode(c["pre_png"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        let raw =
            granary_compat::crypto::pyb64::b64decode_str(c["data"].as_str().unwrap()).unwrap();
        let mine = granary_services::wa_export::shrink_pixels(&raw).unwrap();
        let resize_diff = mean_diff(&mine, &pre);
        // After JPEG: each encoder's loss against Pillow's pre-JPEG pixels, and the two outputs against each other.
        let py_loss = mean_diff(&want, &pre);
        let rs_loss = mean_diff(&got, &pre);
        let between = mean_diff(&got, &want);
        println!("{name}: {}x{} resize diff {resize_diff:.2}, jpeg loss py {py_loss:.2} rs {rs_loss:.2}, outputs {between:.2} (/255)", size.0, size.1);
        assert!(resize_diff < 4.0, "{name}: resize diff {resize_diff}");
        assert!(
            rs_loss <= py_loss * 1.5 + 2.0,
            "{name}: jpeg loss {rs_loss} vs {py_loss}"
        );
    }
}

#[test]
fn golden_build_pdf_images() {
    let mut cases = load("build_pdf_images");
    for c in cases.as_array_mut().unwrap() {
        let got = build_pdf_messages(&arr(&c["messages"]), "Bob", "Them", chrono_tz::Asia::Tokyo)
            .unwrap();
        let got: Vec<Value> = got
            .into_iter()
            .map(|mut g| {
                if let Some(url) = g["image"].as_str() {
                    let img = decode_data_url(url);
                    g["image"] = json!({"size": [img.width(), img.height()]});
                }
                g
            })
            .collect();
        if bless::blessing() {
            c["out"] = Value::Array(got.clone());
        }
        let want = arr(&c["out"]);
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(want.iter()) {
            assert_eq!(g, w);
        }
    }
    bless::save("wa_export/build_pdf_images.json", &cases);
}

fn mean_diff(a: &image::DynamicImage, b: &image::DynamicImage) -> f64 {
    let (a, b) = (a.to_rgb8(), b.to_rgb8());
    assert_eq!(a.dimensions(), b.dimensions());
    let total: u64 = a
        .as_raw()
        .iter()
        .zip(b.as_raw().iter())
        .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64)
        .sum();
    total as f64 / a.as_raw().len() as f64
}
