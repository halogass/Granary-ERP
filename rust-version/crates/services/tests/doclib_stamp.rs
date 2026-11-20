//! doclib A2: the watermark and the hidden fingerprint (stamp.rs), without a database.

use std::io::Cursor;

use granary_services::doc_library::stamp::{
    self, jpeg_comments, png_texts, read_fingerprints, stamp_copy, unzip_all, Stamp, MARK_KEY,
};
use granary_services::later::verification::zip_deflated;
use granary_services::pdftools::{Document, Point, SaveOptions, TextOptions};

const FP: &str = "ID-7K3M9QX2TA";
const FOOTER: &str = "Prepared for Vandelay Industries Ltd · 21 Oct 2026 · Acme Ltd · Confidential";

fn three_page_pdf() -> Vec<u8> {
    let mut d = Document::new().unwrap();
    for i in 0..3 {
        let p = d.new_page(595.0, 842.0).unwrap();
        d.insert_text(
            p,
            Point::new(72.0, 100.0),
            &format!("Brochure page {}", i + 1),
            &TextOptions::default(),
        )
        .unwrap();
    }
    d.tobytes(SaveOptions {
        garbage: 1,
        deflate: true,
        clean: false,
    })
    .unwrap()
}

fn stamp() -> Stamp {
    Stamp::new(FOOTER, FP).with_font_size(7.0)
}

#[test]
fn pdf_footer_on_every_page_bottom_center() {
    let out = stamp_copy(&three_page_pdf(), "application/pdf", &stamp()).unwrap();
    let doc = Document::open(&out).unwrap();
    assert_eq!(doc.page_count(), 3);
    for p in 0..3 {
        let text = doc.get_text(p).unwrap();
        assert!(text.contains(&format!("Brochure page {}", p + 1)), "{text}");
        assert!(text.contains(FOOTER), "page {p}: {text}");
        let page = doc.page_rect(p).unwrap();
        let hits = doc.search_for(p, "Acme Ltd").unwrap();
        assert_eq!(hits.len(), 1, "page {p}");
        let r = hits[0];
        // bottom of the page (y grows downwards), small, not diagonal (one flat line)
        assert!(r.y1 > page.y1 - 20.0 && r.y1 <= page.y1, "page {p}: {r:?}");
        assert!(r.height() < 12.0, "{r:?}");
        // the whole line is centered
        let line = doc.search_for(p, "Prepared for").unwrap()[0];
        let end = doc.search_for(p, "Confidential").unwrap()[0];
        let mid = (line.x0 + end.x1) / 2.0;
        assert!((mid - page.width() / 2.0).abs() < 3.0, "mid {mid}");
        // grey, not black: the footer stream sets a gray fill
        let streams = doc.page_contents(p).unwrap();
        let all: Vec<u8> = streams.concat();
        let s = String::from_utf8_lossy(&all);
        assert!(s.contains("\n.45 g\n"), "{s}");
        assert!(s.contains("/GranaryStampF 7 Tf"), "{s}");
    }
}

#[test]
fn pdf_invisible_fingerprint_and_info_dict() {
    let out = stamp_copy(&three_page_pdf(), "application/pdf", &stamp()).unwrap();
    let mut doc = Document::open(&out).unwrap();
    for p in 0..3 {
        let text = doc.get_text(p).unwrap();
        assert!(
            text.contains(&format!("{MARK_KEY} {FP}")),
            "page {p}: {text}"
        );
        let all = String::from_utf8_lossy(&doc.page_contents(p).unwrap().concat()).into_owned();
        assert!(all.contains("3 Tr"), "render mode 3 (invisible)");
    }
    assert_eq!(doc.get_info(MARK_KEY).unwrap().as_deref(), Some(FP));
    assert!(doc.get_info("Keywords").unwrap().unwrap().contains(FP));
    assert_eq!(read_fingerprints(&out), vec![FP.to_string()]);
}

#[test]
fn pdf_hidden_text_survives_stripped_metadata() {
    let out = stamp_copy(&three_page_pdf(), "application/pdf", &stamp()).unwrap();
    let mut doc = Document::open(&out).unwrap();
    doc.clear_info().unwrap();
    let resaved = doc
        .tobytes(SaveOptions {
            garbage: 4,
            deflate: true,
            clean: true,
        })
        .unwrap();
    let mut again = Document::open(&resaved).unwrap();
    assert_eq!(again.get_info(MARK_KEY).unwrap(), None);
    assert_eq!(read_fingerprints(&resaved), vec![FP.to_string()]);
}

#[test]
fn pdf_restamped_copy_reports_the_newest_mark_first() {
    let first = stamp_copy(&three_page_pdf(), "application/pdf", &stamp()).unwrap();
    let second = stamp_copy(
        &first,
        "application/pdf",
        &Stamp::new("Downloaded by Ana", "ID-0000000001"),
    )
    .unwrap();
    let fps = read_fingerprints(&second);
    assert_eq!(fps.first().map(String::as_str), Some("ID-0000000001"));
    assert!(fps.contains(&FP.to_string()));
}

#[test]
fn pdf_footer_with_non_latin_text_and_long_line() {
    let long = format!("{} · Acme Café 東京 {}", FOOTER, "x".repeat(300));
    let out = stamp_copy(&three_page_pdf(), "application/pdf", &Stamp::new(long, FP)).unwrap();
    let doc = Document::open(&out).unwrap();
    let text = doc.get_text(0).unwrap();
    assert!(text.contains("Acme Café ??"), "{text}");
    // shrunk to fit: still one line inside the page
    let r = doc.search_for(0, "Prepared for").unwrap()[0];
    assert!(r.x0 >= 0.0);
}

fn white_image(w: u32, h: u32) -> image::RgbaImage {
    image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]))
}

fn encode(img: &image::RgbaImage, fmt: image::ImageFormat) -> Vec<u8> {
    let mut out = Vec::new();
    let dynimg = if fmt == image::ImageFormat::Jpeg {
        image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(img.clone()).to_rgb8())
    } else {
        image::DynamicImage::ImageRgba8(img.clone())
    };
    dynimg.write_to(&mut Cursor::new(&mut out), fmt).unwrap();
    out
}

/// Grey pixels in the bottom right corner, the top left untouched.
fn assert_corner_stamp(bytes: &[u8]) {
    let img = image::load_from_memory(bytes).unwrap().to_rgba8();
    let (w, h) = img.dimensions();
    let mut dark = 0;
    for y in h * 3 / 4..h {
        for x in w / 3..w {
            let p = img.get_pixel(x, y);
            if p.0[0] < 200 {
                dark += 1;
            }
        }
    }
    assert!(dark > 20, "no stamp text found ({dark} dark pixels)");
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            assert!(img.get_pixel(x, y).0[0] > 240, "changed at {x},{y}");
        }
    }
}

#[test]
fn png_corner_stamp_and_text_chunk() {
    let png = encode(&white_image(800, 600), image::ImageFormat::Png);
    let out = stamp_copy(&png, "image/png", &stamp()).unwrap();
    assert_corner_stamp(&out);
    assert!(png_texts(&out)
        .iter()
        .any(|(k, v)| k == MARK_KEY && v == FP));
    assert_eq!(read_fingerprints(&out), vec![FP.to_string()]);
}

#[test]
fn jpeg_corner_stamp_and_comment() {
    let jpg = encode(&white_image(800, 600), image::ImageFormat::Jpeg);
    let out = stamp_copy(&jpg, "image/jpeg", &stamp()).unwrap();
    assert_corner_stamp(&out);
    assert!(jpeg_comments(&out).iter().any(|c| c.contains(FP)));
    assert_eq!(read_fingerprints(&out), vec![FP.to_string()]);
}

#[test]
fn webp_and_gif_stamped_and_readable() {
    let webp = encode(&white_image(640, 480), image::ImageFormat::WebP);
    let out = stamp_copy(&webp, "image/webp", &stamp()).unwrap();
    assert_corner_stamp(&out);
    assert_eq!(read_fingerprints(&out), vec![FP.to_string()]);

    let gif = encode(&white_image(640, 480), image::ImageFormat::Gif);
    let out = stamp_copy(&gif, "image/gif", &stamp()).unwrap();
    assert_corner_stamp(&out);
    assert_eq!(read_fingerprints(&out), vec![FP.to_string()]);
}

const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

fn docx(with_custom: bool) -> Vec<u8> {
    let mut entries = vec![
        ("[Content_Types].xml".to_string(), br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>"#.to_vec()),
        ("_rels/.rels".to_string(), br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#.to_vec()),
        ("docProps/core.xml".to_string(), br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Datasheet</dc:title><cp:keywords>laptop</cp:keywords></cp:coreProperties>"#.to_vec()),
        ("word/document.xml".to_string(), br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Hello</w:t></w:r></w:p></w:body></w:document>"#.to_vec()),
    ];
    if with_custom {
        entries.push(("docProps/custom.xml".to_string(), br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="Client"><vt:lpwstr>ACME</vt:lpwstr></property></Properties>"#.to_vec()));
    }
    zip_deflated(&entries, chrono::Local::now())
}

fn part(zip: &[u8], name: &str) -> String {
    let e = unzip_all(zip).unwrap();
    String::from_utf8(e.into_iter().find(|(n, _)| n == name).unwrap().1).unwrap()
}

#[test]
fn ooxml_custom_property_no_visible_change() {
    let src = docx(false);
    let out = stamp_copy(&src, DOCX_MIME, &stamp()).unwrap();
    let custom = part(&out, "docProps/custom.xml");
    assert!(custom.contains(&format!("name=\"{MARK_KEY}\"")), "{custom}");
    assert!(custom.contains(FP));
    assert!(part(&out, "[Content_Types].xml").contains("/docProps/custom.xml"));
    assert!(part(&out, "_rels/.rels").contains("custom-properties"));
    let core = part(&out, "docProps/core.xml");
    assert!(
        core.contains("laptop; GranaryFingerprint ID-7K3M9QX2TA"),
        "{core}"
    );
    assert_eq!(
        part(&out, "word/document.xml"),
        part(&src, "word/document.xml")
    );
    assert_eq!(read_fingerprints(&out), vec![FP.to_string()]);
    // the text reader of batch 4 still opens it
    assert!(
        granary_services::nary::file_text::zip_entry(&out, "word/document.xml", 1 << 20)
            .unwrap()
            .is_some()
    );
}

#[test]
fn ooxml_keeps_existing_custom_properties() {
    let out = stamp_copy(&docx(true), DOCX_MIME, &stamp()).unwrap();
    let custom = part(&out, "docProps/custom.xml");
    assert!(custom.contains("name=\"Client\""), "{custom}");
    assert!(custom.contains("pid=\"3\""), "{custom}");
    assert!(custom.contains(FP));
    // stamping again replaces our property instead of adding a second one
    let again = stamp_copy(&out, DOCX_MIME, &Stamp::new("", "ID-0000000002")).unwrap();
    let custom = part(&again, "docProps/custom.xml");
    assert_eq!(custom.matches(MARK_KEY).count(), 1, "{custom}");
    assert_eq!(read_fingerprints(&again)[0], "ID-0000000002");
}

#[test]
fn unknown_types_are_refused() {
    let e = stamp_copy(b"hello", "text/plain", &stamp()).unwrap_err();
    assert!(format!("{e:?}").contains("can't be watermarked"), "{e:?}");
    assert!(stamp::read_fingerprints(b"plain text").is_empty());
}

/// A hand-written one-page PDF with /Rotate (MuPDF rebuilds the missing xref).
fn rotated_pdf(rotate: i32) -> Vec<u8> {
    format!(
        "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Rotate {rotate} /Resources << >> >> endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF\n"
    )
    .into_bytes()
}

#[test]
fn pdf_footer_follows_the_page_rotation() {
    for rot in [0, 90, 180, 270] {
        let out = stamp_copy(&rotated_pdf(rot), "application/pdf", &stamp()).unwrap();
        let doc = Document::open(&out).unwrap();
        let page = doc.page_rect(0).unwrap();
        let hits = doc.search_for(0, "Acme Ltd").unwrap();
        assert_eq!(hits.len(), 1, "rotate {rot}");
        let r = hits[0];
        assert!(
            r.y1 > page.y1 - 20.0 && r.y1 <= page.y1 + 0.5 && r.height() < 12.0,
            "rotate {rot}: {r:?} in {page:?}"
        );
        let first = doc.search_for(0, "Prepared for").unwrap()[0];
        let last = doc.search_for(0, "Confidential").unwrap()[0];
        assert!(first.x0 < last.x0, "reads left to right at rotate {rot}");
        let mid = (first.x0 + last.x1) / 2.0;
        assert!(
            (mid - page.width() / 2.0).abs() < 3.0,
            "rotate {rot}: mid {mid} of {page:?}"
        );
    }
}
