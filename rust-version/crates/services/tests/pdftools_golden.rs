//! Golden tests for crates/services/src/pdftools.rs. The fixtures were first recorded from PyMuPDF 1.24.14
//! (compat-tests/golden/gen_pdftools.py) and re-recorded when MuPDF was replaced by lopdf + hayro. The
//! content streams written by the drawing operations, operation return values, text lengths and page
//! subsetting are exact. Text extraction and search follow the same line and spacing rules as MuPDF on
//! these fixtures; search rectangles depend on font extents and are compared to 1e-3 pt. Page renders of
//! the flattened output are compared pixel by pixel with a small anti-aliasing tolerance.
//! With `GOLDEN_BLESS=1` the expectations, the flattened PDFs and the page renders re-record from this
//! code (tests/golden/bless.rs): it is now the reference.

#[path = "golden/bless.rs"]
mod bless;

use std::path::PathBuf;

use granary_services::pdftools::{
    get_text_length, Document, Point, Rect, SaveOptions, TextOptions,
};
use serde_json::Value;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../compat-tests/golden/pdftools")
}

fn cases() -> Value {
    bless::read("pdftools/cases.json")
}

/// Saves the blessed cases (tests of this binary that bless the same file take turns).
fn save_cases(f: impl FnOnce(&mut Value)) {
    bless::update("pdftools/cases.json", f);
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

fn rect(v: &Value) -> Rect {
    let a = v.as_array().unwrap();
    Rect::new(f(&a[0]), f(&a[1]), f(&a[2]), f(&a[3]))
}

fn apply(doc: &mut Document, op: &Value) -> Value {
    let pno = op["page"].as_u64().unwrap() as usize;
    match op["op"].as_str().unwrap() {
        "insert_textbox" => {
            let opts = TextOptions {
                fontsize: f(&op["fontsize"]),
                fontname: op
                    .get("fontname")
                    .and_then(|v| v.as_str())
                    .unwrap_or("helv")
                    .to_string(),
                color: op["color"].as_array().map(|a| a.iter().map(f).collect()),
                align: op["align"].as_i64().unwrap() as i32,
            };
            let rc = doc
                .insert_textbox(pno, rect(&op["rect"]), op["text"].as_str().unwrap(), &opts)
                .unwrap();
            serde_json::json!(rc)
        }
        "draw_line" => {
            let p1 = op["p1"].as_array().unwrap();
            let p2 = op["p2"].as_array().unwrap();
            let color: Vec<f64> = op["color"].as_array().unwrap().iter().map(f).collect();
            doc.draw_line(
                pno,
                Point::new(f(&p1[0]), f(&p1[1])),
                Point::new(f(&p2[0]), f(&p2[1])),
                Some(&color),
                f(&op["width"]),
            )
            .unwrap();
            Value::Null
        }
        "insert_image" => {
            let img =
                std::fs::read(dir().join("images").join(op["image"].as_str().unwrap())).unwrap();
            doc.insert_image(pno, rect(&op["rect"]), &img).unwrap();
            Value::Null
        }
        "insert_text" => {
            let p = op["point"].as_array().unwrap();
            let opts = TextOptions {
                fontsize: f(&op["fontsize"]),
                ..Default::default()
            };
            serde_json::json!(doc
                .insert_text(
                    pno,
                    Point::new(f(&p[0]), f(&p[1])),
                    op["text"].as_str().unwrap(),
                    &opts
                )
                .unwrap())
        }
        other => panic!("unknown op {other}"),
    }
}

fn load_png(path: &PathBuf) -> (u32, u32, Vec<u8>) {
    let img = image::open(path).unwrap().to_rgb8();
    (img.width(), img.height(), img.into_raw())
}

/// (max channel difference, number of pixels differing by more than 8 in a channel)
fn diff(a: &(u32, u32, Vec<u8>), w: u32, h: u32, b: &[u8]) -> (u8, usize) {
    assert_eq!((a.0, a.1), (w, h), "render size");
    let mut max = 0u8;
    let mut n = 0usize;
    for (pa, pb) in a.2.chunks(3).zip(b.chunks(3)) {
        let d = pa
            .iter()
            .zip(pb)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap();
        if d > 8 {
            n += 1;
        }
        max = max.max(d);
    }
    (max, n)
}

#[test]
fn flattening_matches_golden() {
    let mut c = cases();
    let zoom = f(&c["zoom"]);
    let mut worst = 0u8;
    let mut off = 0usize;
    for case in c["cases"].as_array_mut().unwrap() {
        let name = case["name"].as_str().unwrap();
        let input =
            std::fs::read(dir().join("inputs").join(case["input"].as_str().unwrap())).unwrap();
        let mut doc = Document::open(&input).unwrap();
        let name = name.to_string();
        let name = name.as_str();
        let ops = case["ops"].as_array().unwrap().clone();
        for (i, op) in ops.iter().enumerate() {
            let got = apply(&mut doc, op);
            bless::eq(
                &mut case["results"][i],
                got,
                format!("{name}: return value of {op}"),
            );
        }
        let pages: Vec<String> = case["streams"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for pno in pages {
            let streams = &mut case["streams"][&pno];
            let got: Vec<String> = doc
                .page_contents(pno.parse().unwrap())
                .unwrap()
                .iter()
                .map(|s| s.iter().map(|b| *b as char).collect())
                .collect();
            bless::eq(
                streams,
                got,
                format!("{name}: content streams of page {pno}"),
            );
        }
        let out = doc
            .tobytes(SaveOptions {
                garbage: 4,
                deflate: true,
                clean: true,
            })
            .unwrap();
        let flat = Document::open(&out).unwrap();
        if bless::blessing() {
            std::fs::write(dir().join("flattened").join(format!("{name}.pdf")), &out).unwrap();
        }
        let n_flat = case["flat_text"].as_array().unwrap().len();
        for pno in 0..n_flat {
            bless::eq(
                &mut case["flat_text"][pno],
                flat.get_text(pno).unwrap(),
                format!("{name}: text of flattened page {pno}"),
            );
        }
        for (pno, fname) in case["renders"].as_array().unwrap().iter().enumerate() {
            if bless::blessing() {
                let img = flat.render_page(pno, zoom).unwrap();
                image::RgbImage::from_raw(img.width, img.height, img.samples)
                    .unwrap()
                    .save(dir().join("renders").join(fname.as_str().unwrap()))
                    .unwrap();
            }
            let expected = load_png(&dir().join("renders").join(fname.as_str().unwrap()));
            let ours = flat.render_page(pno, zoom).unwrap();
            let (max, n) = diff(&expected, ours.width, ours.height, &ours.samples);
            eprintln!("{name} page {pno}: render vs golden max channel diff {max} ({n} px over 8)");
            worst = worst.max(max);
            // the renderer is deterministic; other CPU feature levels may anti-alias a few edge pixels differently
            assert!(
                n * 1000 <= (ours.width * ours.height) as usize,
                "{name} page {pno}: {n} pixels differ from the golden render"
            );
            off += n;
        }
    }
    let cases_now = c["cases"].clone();
    save_cases(|v| v["cases"] = cases_now);
    eprintln!("WORST max channel diff: {worst}; pixels over 8: {off}");
}

#[test]
fn text_extraction_and_search_match_golden() {
    let mut c = cases();
    for (src, exp) in c["extraction"].as_object_mut().unwrap() {
        let doc = Document::open(&std::fs::read(dir().join("inputs").join(src)).unwrap()).unwrap();
        assert_eq!(
            doc.page_count() as u64,
            exp["page_count"].as_u64().unwrap(),
            "{src}"
        );
        for (pno, r) in exp["rects"].as_array().unwrap().iter().enumerate() {
            assert_eq!(doc.page_rect(pno).unwrap(), rect(r), "{src} rect {pno}");
        }
        let n_text = exp["text"].as_array().unwrap().len();
        for pno in 0..n_text {
            bless::eq(
                &mut exp["text"][pno],
                doc.get_text(pno).unwrap(),
                format!("{src} text page {pno}"),
            );
        }
        for (needle, pages) in exp["search"].as_object_mut().unwrap() {
            for (pno, hits) in pages.as_array_mut().unwrap().iter_mut().enumerate() {
                let got = doc.search_for(pno, needle).unwrap();
                if bless::blessing() {
                    *hits = serde_json::json!(got
                        .iter()
                        .map(|r| [r.x0, r.y0, r.x1, r.y1])
                        .collect::<Vec<_>>());
                    continue;
                }
                let want: Vec<Rect> = hits.as_array().unwrap().iter().map(rect).collect();
                assert_eq!(
                    got.len(),
                    want.len(),
                    "{src} search {needle:?} page {pno}: {got:?} vs {want:?}"
                );
                for (g, w) in got.iter().zip(&want) {
                    let dev = [g.x0 - w.x0, g.y0 - w.y0, g.x1 - w.x1, g.y1 - w.y1]
                        .iter()
                        .fold(0f64, |a, b| a.max(b.abs()));
                    assert!(
                        dev < 1e-3,
                        "{src} search {needle:?} page {pno}: {g:?} vs {w:?}"
                    );
                }
            }
        }
    }
    let extraction = c["extraction"].clone();
    save_cases(|v| v["extraction"] = extraction);
}

#[test]
fn text_length_matches_golden() {
    let mut c = cases();
    for case in c["lengths"].as_array_mut().unwrap() {
        let got = get_text_length(
            case["text"].as_str().unwrap(),
            case["fontname"].as_str().unwrap(),
            f(&case["fontsize"]),
        )
        .unwrap();
        if bless::blessing() {
            case["length"] = serde_json::json!(got);
        }
        assert_eq!(got, f(&case["length"]), "{case}");
    }
    let lengths = c["lengths"].clone();
    save_cases(|v| v["lengths"] = lengths);
}

#[test]
fn subsetting_matches_golden() {
    let mut c = cases();
    let src = Document::open(&std::fs::read(dir().join("inputs/text.pdf")).unwrap()).unwrap();
    for s in c["subsets"].as_array_mut().unwrap() {
        let mut out = Document::new().unwrap();
        out.insert_pdf(
            &src,
            s["first"].as_u64().unwrap() as usize,
            s["last"].as_u64().unwrap() as usize,
        )
        .unwrap();
        let bytes = out.tobytes(SaveOptions::default()).unwrap();
        let sub = Document::open(&bytes).unwrap();
        assert_eq!(sub.page_count() as u64, s["page_count"].as_u64().unwrap());
        let n_text = s["text"].as_array().unwrap().len();
        for pno in 0..n_text {
            bless::eq(
                &mut s["text"][pno],
                sub.get_text(pno).unwrap(),
                format!("subset page {pno}"),
            );
        }
    }
    let subsets = c["subsets"].clone();
    save_cases(|v| v["subsets"] = subsets);
}

#[test]
fn open_errors_match_golden() {
    let c = cases();
    for (label, blob) in [
        ("garbage", &b"hello"[..]),
        ("empty", &b""[..]),
        ("pdf_header_only", &b"%PDF-1.4 garbage"[..]),
    ] {
        let got = Document::open(blob).err().map(|e| e.0);
        assert_eq!(got.as_deref(), c["open_errors"][label].as_str(), "{label}");
    }
    assert_eq!(
        Document::new()
            .unwrap()
            .tobytes(SaveOptions::default())
            .unwrap_err()
            .0,
        "cannot save with zero pages"
    );
}
