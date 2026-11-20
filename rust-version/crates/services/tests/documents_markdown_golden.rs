//! Golden: documents/render.rs Markdown against markdown-it-py 3.0.0
//! (compat-tests/golden/documents/markdown.json, recorded by compat-tests/golden/gen_documents.py).
use granary_services::documents::render;

#[derive(serde::Deserialize)]
struct Case {
    md: String,
    raw: String,
    html: String,
}

fn cases() -> Vec<Case> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../compat-tests/golden/documents/markdown.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn check(name: &str, f: impl Fn(&Case) -> (String, &String)) {
    let all = cases();
    let mut bad = 0;
    for (i, c) in all.iter().enumerate() {
        let (got, want) = f(c);
        if &got != want {
            bad += 1;
            if bad <= 20 {
                eprintln!("{name} #{i} md={:?}\n want={want:?}\n  got={got:?}\n", c.md);
            }
        }
    }
    assert_eq!(bad, 0, "{name}: {bad} of {} differ", all.len());
}

/// `_MD.render(md)`: CommonMark 0.30 spec examples, hand-written cases and fuzz.
#[test]
fn md_render_matches_markdown_it_py() {
    check("raw", |c| (render::md_render(&c.md), &c.raw));
}

/// `markdown_html(md)`: the render through verification.render.sanitize_html.
#[test]
fn markdown_html_matches_python() {
    check("html", |c| (render::markdown_html(Some(&c.md)), &c.html));
}
