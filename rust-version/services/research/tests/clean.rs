//! Port of services/research/tests/test_clean.py: rendered pages -> text worth sending to a model.

use granary_research::clean::{clean_markdown, html_to_markdown, links, title_of};

const PAGE: &str = "[![Astra](data:image/svg+xml,%3csvg%20width='137'%3e%3c/svg%3e)](/)
- [Home](/)
- [About](/about)
- [Careers](/careers)
- [Contact Us](/contact)
- |

# Board of Directors

## [Robert Smith](/board/profile?index=0)

### President Director

Globex Construction Corp is a state-owned construction company headquartered in Tokyo.

![photo](https://x.test/p.jpg)

Copyright Globex Construction Corp 2026. All rights reserved and more words here.
Copyright Globex Construction Corp 2026. All rights reserved and more words here.
";

#[test]
fn test_menus_logos_and_images_go_but_the_content_stays() {
    let text = clean_markdown(PAGE, 12000);
    assert!(!text.contains("data:image") && !text.contains("Careers") && !text.contains("p.jpg"));
    assert!(
        text.contains("Robert Smith"),
        "a heading that is a link keeps its text"
    );
    assert!(text.contains("President Director") && text.contains("state-owned construction"));
    assert_eq!(
        text.matches("Copyright Globex Construction Corp").count(),
        1,
        "repeated footer text appears once"
    );
}

#[test]
fn test_links_are_absolute_and_skip_images_and_mail() {
    let md = format!("{PAGE}\n[Mail](mailto:a@b.c) [Top](#top)");
    let found = links(&md, "https://www.wika.example/en/company/").unwrap();
    let hrefs: Vec<&str> = found.iter().map(|l| l["href"].as_str().unwrap()).collect();
    assert!(hrefs.contains(&"https://www.wika.example/about"));
    assert!(hrefs.contains(&"https://www.wika.example/board/profile?index=0"));
    assert!(!hrefs
        .iter()
        .any(|h| h.starts_with("mailto:") || h.starts_with("data:") || h.ends_with("#top")));
}

#[test]
fn test_length_is_capped() {
    assert!(clean_markdown(&"word ".repeat(10000), 800).chars().count() <= 800);
}

#[test]
fn test_title_is_the_first_heading() {
    assert_eq!(title_of(PAGE).as_deref(), Some("Board of Directors"));
}

#[test]
fn test_the_plain_fetch_converter_keeps_structure_and_links() {
    let html = r#"<html><head><title>Northwind Steel</title><script>var x=1</script></head><body>
      <nav><ul><li><a href="/a">A</a></li></ul></nav>
      <h1>About Us</h1><p>Northwind Steel is a <a href="/products">maker</a> of steel in Osaka.</p>
      <address>1 Main Street, Osaka</address></body></html>"#;
    let (markdown, title) = html_to_markdown(html);
    assert_eq!(title.as_deref(), Some("Northwind Steel"));
    assert!(markdown.contains("# About Us") && markdown.contains("[maker](/products)"));
    assert!(markdown.contains("1 Main Street, Osaka") && !markdown.contains("var x"));
}
