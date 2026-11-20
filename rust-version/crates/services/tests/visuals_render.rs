//! Proposal figures (visuals V1): golden SVG per kind, guard cases, PNG size, escaping, custom SVG
//! sanitiser. Goldens live in tests/golden/visuals/; regenerate with VISUALS_UPDATE=1. Set
//! VISUALS_PNG_DIR=<dir> to also write a PNG of every sample for a visual review.

use std::path::PathBuf;

use granary_services::visuals::guard::requirement_numbers;
use granary_services::visuals::{
    self, guard, render_png, render_svg, render_svg_sized, sanitize, CatalogProduct, ErpData,
    FigureBlock, FigureSize, GuardContext, Spec,
};
use serde_json::{json, Value};

fn spec(v: Value) -> Spec {
    Spec::from_value(v).expect("spec parses")
}

fn golden(name: &str, svg: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/visuals");
    let path = dir.join(format!("{name}.svg"));
    if std::env::var("VISUALS_UPDATE").is_ok() {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, svg).unwrap();
    } else {
        let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "missing golden {}; run with VISUALS_UPDATE=1",
                path.display()
            )
        });
        assert_eq!(
            svg, want,
            "golden {name} differs; inspect and rerun with VISUALS_UPDATE=1"
        );
    }
    if let Ok(out) = std::env::var("VISUALS_PNG_DIR") {
        let png = visuals::raster::svg_to_png(svg, 1360).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(PathBuf::from(out).join(format!("{name}.png")), png).unwrap();
    }
}

fn customer() -> Value {
    json!({"type": "customer", "note": "from the customer's site list, Oct 2026"})
}

fn samples() -> Vec<(&'static str, Value)> {
    vec![
        (
            "gantt",
            json!({"kind": "gantt", "title": "Project timeline", "language": "en", "time_unit": "week",
            "source": {"type": "erp", "ref": "proposal.phases"},
            "tasks": [
                {"name": "Site survey", "start": 0, "length": 1, "group": "Preparation"},
                {"name": "Hardware procurement and shipping", "start": 1, "length": 3, "group": "Preparation"},
                {"name": "Software configuration", "start": 1, "length": 2, "group": "Software"},
                {"name": "Installation at three offices", "start": 4, "length": 1.5, "group": "Installation"},
                {"name": "Training and handover", "start": 5.5, "length": 1, "group": "Installation"}],
            "milestones": [{"name": "Kick-off", "at": 0}, {"name": "Go-live", "at": 6.5}],
            "categories": [], "series": []}),
        ),
        (
            "bar",
            json!({"kind": "bar", "title": "Users per site", "language": "en", "unit": "users",
            "categories": ["Tokyo HQ", "Osaka warehouse", "Yokohama plant", "Sapporo branch"],
            "series": [{"name": "Existing", "values": [24, 12, 40, 8]}, {"name": "New", "values": [6, 10, 12, 4]}],
            "source": customer()}),
        ),
        (
            "stacked_bar",
            json!({"kind": "stacked_bar", "title": "Rollout by month", "language": "en", "unit": "users",
            "categories": ["Nov", "Dec", "Jan", "Feb", "Mar"],
            "series": [{"name": "Office users", "values": [4, 8, 8, 6, 2]}, {"name": "Field users", "values": [2, 6, 10, 10, 4]}],
            "source": customer()}),
        ),
        (
            "line",
            json!({"kind": "line", "title": "Visitors per day (customer count)", "language": "en", "unit": "visitors",
            "categories": ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
            "series": [{"name": "Visitors", "values": [1200, 1350, 1280, 1500, 2100, 3400, 3100]}],
            "annotations": [{"type": "ref_line", "axis": "y", "value": 2500, "label": "Peak staffing threshold"},
                            {"type": "highlight", "series": "Visitors", "index": 5, "label": "Weekend peak"}],
            "source": customer()}),
        ),
        (
            "area",
            json!({"kind": "area", "title": "Orders per hour", "language": "en", "unit": "orders",
            "categories": ["06", "07", "08", "09", "10", "11", "12", "13", "14", "15", "16", "17", "18"],
            "series": [{"name": "In", "values": [40, 120, 210, 160, 90, 70, 60, 80, 90, 100, 150, 230, 140]},
                       {"name": "Out", "values": [10, 30, 50, 60, 70, 60, 80, 70, 80, 120, 200, 260, 180]}],
            "source": {"type": "analysis", "note": "counted from 7 days of customer data", "from": ["read_attachment:abc"]}}),
        ),
        (
            "donut",
            json!({"kind": "donut", "title": "Licence mix", "language": "en", "unit": "licences",
            "categories": ["Full", "Read-only", "Approver", "Portal"], "series": [{"name": "Licences", "values": [38, 22, 4, 6]}],
            "source": customer()}),
        ),
        (
            "milestones",
            json!({"kind": "milestones", "title": "Key dates", "language": "en", "time_unit": "week",
            "milestones": [{"name": "Contract signed", "at": 0}, {"name": "Hardware on site", "at": 3},
                           {"name": "Pilot team live", "at": 4}, {"name": "All teams live", "at": 6}, {"name": "Handover", "at": 7}],
            "categories": [], "series": [], "source": {"type": "erp", "ref": "proposal.phases"}}),
        ),
        (
            "hbar",
            json!({"kind": "hbar", "title": "Support tickets per team (last 6 months)", "language": "en", "unit": "tickets",
            "categories": ["Sales", "Warehouse", "Finance", "HR", "Operations"],
            "series": [{"name": "Tickets", "values": [14, 9, 5, 3, 2]}],
            "annotations": [{"type": "ref_line", "axis": "y", "value": 6, "label": "Target"}],
            "source": customer()}),
        ),
        (
            "combo",
            json!({"kind": "combo", "title": "Tickets and support hours", "language": "en",
            "categories": ["Jan", "Feb", "Mar", "Apr", "May", "Jun"],
            "series": [{"name": "Tickets", "values": [12, 10, 9, 7, 5, 4], "type": "bar"},
                       {"name": "Support hours", "values": [320, 300, 260, 240, 200, 180], "type": "line", "axis": "right"}],
            "y_label": "Tickets", "source": customer()}),
        ),
        (
            "scatter",
            json!({"kind": "scatter", "title": "Order volume and handling time", "language": "en",
            "x_label": "Orders per hour", "y_label": "Average handling (minutes)",
            "series": [{"name": "Branches", "values": [], "points": [
                {"x": 120, "y": 4.5, "label": "Branch A"}, {"x": 200, "y": 8, "label": "Branch B"},
                {"x": 80, "y": 2, "label": "Branch C"}, {"x": 260, "y": 11.5, "label": "Branch D"}, {"x": 150, "y": 5, "label": "Branch E"}]}],
            "categories": [], "source": customer()}),
        ),
        (
            "heatmap",
            json!({"kind": "heatmap", "title": "Visitors by hour and day", "language": "en", "unit": "visitors",
            "categories": ["08", "10", "12", "14", "16", "18", "20"],
            "series": [{"name": "Weekday", "values": [40, 80, 160, 120, 140, 210, 90]},
                       {"name": "Saturday", "values": [60, 140, 260, 300, 280, 320, 200]},
                       {"name": "Sunday", "values": [50, 120, 240, 280, 260, 300, 150]}],
            "source": customer()}),
        ),
        (
            "waterfall",
            json!({"kind": "waterfall", "title": "Manual checking hours per week", "language": "en", "unit": "hours",
            "categories": ["Today", "Automatic plate log", "Face check-in", "Alert triage", "New reports"],
            "series": [{"name": "Hours", "values": [120, -45, -30, -15, 6]}],
            "source": {"type": "analysis", "note": "from the customer's staff roster and shift log", "from": ["read_attachment:roster"]}}),
        ),
        (
            "radar",
            json!({"kind": "radar", "title": "Site readiness", "language": "en",
            "categories": ["Power", "Network", "Mounting points", "Lighting", "Cabinet space", "Access"],
            "series": [{"name": "Site A", "values": [4, 3, 5, 2, 4, 5]}, {"name": "Site B", "values": [2, 4, 3, 4, 2, 3]}],
            "source": {"type": "analysis", "note": "survey scores from the site visit, 1 to 5", "from": ["get_deal"]}}),
        ),
        (
            "architecture",
            json!({"kind": "architecture", "title": "System architecture", "language": "en", "direction": "LR",
            "groups": [{"id": "site", "label": "Customer office (on-premise)", "style": "site"},
                       {"id": "hr", "label": "Customer systems", "style": "customer_system"}],
            "nodes": [
                {"id": "pc", "label": "Staff laptops", "icon": "laptop", "sku": null, "group": "site", "count": 12},
                {"id": "ph", "label": "Phones", "icon": "phone", "sku": null, "group": "site", "count": 3},
                {"id": "sw", "label": "Office switch", "icon": "networkSwitch", "sku": null, "group": "site", "count": 1},
                {"id": "box", "label": "Edge Gateway", "icon": "server", "sku": "EGW-1", "group": "site", "count": 1},
                {"id": "erp", "label": "ERPNext (attendance)", "icon": "erp", "sku": null, "group": "hr", "count": 1},
                {"id": "door", "label": "Door controller", "icon": "door", "sku": null, "group": "hr", "count": 1}],
            "edges": [
                {"from": "pc", "to": "sw", "kind": "ethernet", "label": null},
                {"from": "ph", "to": "sw", "kind": "wifi", "label": null},
                {"from": "sw", "to": "box", "kind": "ethernet", "label": "1 Gbps"},
                {"from": "box", "to": "erp", "kind": "api", "label": "REST API"},
                {"from": "box", "to": "door", "kind": "relay", "label": "Open door"}]}),
        ),
        (
            "network",
            json!({"kind": "network", "title": "Network: two sites and the cloud", "language": "en", "direction": "LR",
            "groups": [{"id": "a", "label": "Site A, Tokyo", "style": "site"},
                       {"id": "b", "label": "Site B, Yokohama", "style": "site"},
                       {"id": "c", "label": "Cloud", "style": "cloud"}],
            "nodes": [
                {"id": "ca", "label": "Laptops", "icon": "laptop", "sku": null, "group": "a", "count": 8},
                {"id": "ra", "label": "4G router", "icon": "router4g", "sku": null, "group": "a", "count": 1},
                {"id": "cb", "label": "Laptops", "icon": "laptop", "sku": null, "group": "b", "count": 16},
                {"id": "sb", "label": "Switch", "icon": "networkSwitch", "sku": null, "group": "b", "count": 1},
                {"id": "pw", "label": "Local file server", "icon": "server", "sku": null, "group": "a", "count": 1},
                {"id": "dash", "label": "Dashboard", "icon": "cloud", "sku": null, "group": "c", "count": 1},
                {"id": "user", "label": "Operations team", "icon": "user", "sku": null, "group": null, "count": 1}],
            "edges": [
                {"from": "ca", "to": "ra", "kind": "wifi", "label": null},
                {"from": "pw", "to": "ra", "kind": "ethernet", "label": null},
                {"from": "ra", "to": "dash", "kind": "4g", "label": null},
                {"from": "cb", "to": "sb", "kind": "ethernet", "label": null},
                {"from": "sb", "to": "dash", "kind": "fiber", "label": "Fibre uplink"},
                {"from": "dash", "to": "user", "kind": "data", "label": "Reports"}]}),
        ),
        (
            "process",
            json!({"kind": "process", "title": "From request to approval", "language": "en", "direction": "LR",
            "groups": [{"id": "sys", "label": "System", "style": "lane"}, {"id": "ops", "label": "Finance team", "style": "lane"}],
            "nodes": [
                {"id": "d", "label": "Employee submits a purchase request", "icon": "laptop", "sku": null, "group": "sys", "count": 1},
                {"id": "a", "label": "Notification with the details", "icon": "sensor", "sku": null, "group": "sys", "count": 1},
                {"id": "r", "label": "Manager reviews", "icon": "user", "sku": null, "group": "ops", "count": 1},
                {"id": "f", "label": "Employee is told", "icon": "person", "sku": null, "group": "ops", "count": 1},
                {"id": "l", "label": "Logged in the weekly report", "icon": "database", "sku": null, "group": "sys", "count": 1}],
            "edges": [{"from": "d", "to": "a", "kind": "data", "label": null}, {"from": "a", "to": "r", "kind": "data", "label": "WhatsApp"},
                      {"from": "r", "to": "f", "kind": "data", "label": null}, {"from": "f", "to": "l", "kind": "data", "label": null}]}),
        ),
        (
            "custom_svg",
            json!({"kind": "custom_svg", "title": "Cabinet sketch", "language": "en",
            "svg": "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 200 120\"><rect x=\"10\" y=\"10\" width=\"80\" height=\"100\" rx=\"4\" fill=\"#eee\" stroke=\"#333\"/><text x=\"50\" y=\"65\" font-size=\"10\" text-anchor=\"middle\">Cabinet</text><line x1=\"90\" y1=\"60\" x2=\"180\" y2=\"60\" stroke=\"#c8000a\" marker-end=\"url(#m)\"/></svg>",
            "source": null}),
        ),
    ]
}

#[test]
fn golden_every_kind_full_width() {
    for (name, v) in samples() {
        let s = spec(v);
        let svg = render_svg(&s).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 "),
            "{name}"
        );
        assert!(!svg.contains("href=\"http"), "{name}: external ref");
        assert!(!svg.contains('\u{2014}'), "{name}: em dash");
        golden(name, &svg);
    }
}

#[test]
fn golden_half_width() {
    for (name, v) in samples() {
        if ![
            "gantt",
            "architecture",
            "bar",
            "donut",
            "process",
            "milestones",
            "hbar",
        ]
        .contains(&name)
        {
            continue;
        }
        let s = spec(v);
        let svg = render_svg_sized(&s, FigureSize::Half).unwrap();
        assert!(svg.contains("viewBox=\"0 0 400 "), "{name}: half canvas");
        golden(&format!("{name}_half"), &svg);
    }
}

#[test]
fn rendering_is_deterministic() {
    for (name, v) in samples() {
        let s = spec(v);
        assert_eq!(render_svg(&s).unwrap(), render_svg(&s).unwrap(), "{name}");
    }
}

#[test]
fn png_decodes_with_expected_size() {
    let s = spec(samples()[0].1.clone());
    let png = render_png(&s, 800).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let img = image::load_from_memory(&png).unwrap();
    assert_eq!(img.width(), 800);
    let svg = render_svg(&s).unwrap();
    // Height follows the viewBox aspect ratio.
    let vb: Vec<f64> = svg
        .split("viewBox=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .split(' ')
        .map(|x| x.parse().unwrap())
        .collect();
    let want = (vb[3] * 800.0 / vb[2]).round() as u32;
    assert_eq!(img.height(), want);
    assert!(render_png(&s, 0).is_err());
}

#[test]
fn labels_are_escaped_as_text() {
    let s = spec(
        json!({"kind": "bar", "title": "<script>alert(1)</script> & co", "language": "en",
        "categories": ["A & B", "<b>bold</b>"], "series": [{"name": "x\"y", "values": [1, 2]}],
        "source": customer()}),
    );
    let svg = render_svg(&s).unwrap();
    assert!(!svg.contains("<script>"));
    assert!(!svg.contains("<b>"));
    assert!(svg.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; co"));
    assert!(svg.contains("A &amp; B"));
    let d = spec(
        json!({"kind": "architecture", "title": "t", "nodes": [{"id": "a", "label": "<script>x</script>", "icon": "server"}], "edges": []}),
    );
    let svg = render_svg(&d).unwrap();
    assert!(!svg.contains("<script>"));
    assert!(svg.contains("&lt;script&gt;"));
    // The PNG still renders (the SVG is well formed).
    assert!(render_png(&d, 300).is_ok());
}

#[test]
fn spec_roundtrip_and_block() {
    for (name, v) in samples() {
        let s = spec(v);
        let back = Spec::from_value(s.to_value()).unwrap();
        assert_eq!(s, back, "{name}");
    }
    let b: FigureBlock = serde_json::from_value(
        json!({"spec": samples()[0].1, "caption": "Timeline", "size": "half"}),
    )
    .unwrap();
    assert_eq!(b.size, FigureSize::Half);
    assert_eq!(b.spec.block_type(), "chart");
    assert!(Spec::from_block("diagram", samples()[0].1.clone()).is_err());
    assert!(Spec::from_value(json!({"kind": "pie"}))
        .unwrap_err()
        .contains("unknown kind"));
    // The model schemas are valid JSON objects naming every kind.
    let sc = visuals::spec::spec_schema().to_string();
    for k in visuals::spec::CHART_KINDS
        .iter()
        .chain(visuals::spec::DIAGRAM_KINDS.iter())
    {
        assert!(sc.contains(&format!("\"{k}\"")), "{k}");
    }
    // They pass the Anthropic structured-output rules as written.
    for s in [
        visuals::spec::chart_schema(),
        visuals::spec::diagram_schema(),
        visuals::spec::custom_svg_schema(),
    ] {
        assert!(
            granary_services::ai::schema::violations(&s).is_empty(),
            "{:?}",
            granary_services::ai::schema::violations(&s)
        );
    }
}

// ---------------------------------------------------------------------------------------------------------
// Guard
// ---------------------------------------------------------------------------------------------------------

fn ctx() -> GuardContext {
    GuardContext {
        catalog: vec![
            CatalogProduct {
                sku: "EGW-1".into(),
                name: "Edge Gateway".into(),
                short_name: Some("Edge Gateway".into()),
                family: Some("gateway".into()),
            },
            CatalogProduct {
                sku: "DESK-1".into(),
                name: "Service Desk Suite".into(),
                short_name: Some("Service Desk".into()),
                family: Some("software".into()),
            },
        ],
        ..Default::default()
    }
    .with_erp(
        "proposal.phases",
        ErpData {
            tasks: vec![visuals::GanttTask {
                name: "Survey".into(),
                start: 0.0,
                length: 2.0,
                group: None,
            }],
            milestones: vec![visuals::Milestone {
                name: "Go-live".into(),
                at: 2.0,
            }],
            time_unit: Some(visuals::TimeUnit::Week),
            ..Default::default()
        },
    )
}

fn check(v: Value) -> (Spec, visuals::GuardReport) {
    let mut s = spec(v);
    let r = guard(&mut s, &ctx());
    (s, r)
}

#[test]
fn guard_accepts_every_sample() {
    for (name, v) in samples() {
        let (_, r) = check(v);
        assert!(r.ok(), "{name}: {:?}", r.errors);
    }
}

#[test]
fn guard_refuses_money() {
    for (field, val) in [
        ("unit", json!("JPY")),
        ("title", json!("Cost in JPY per site")),
        ("title", json!("Savings $40k")),
        ("y_label", json!("USD per month")),
    ] {
        let mut v = json!({"kind": "bar", "title": "t", "categories": ["a"], "series": [{"name": "s", "values": [1]}], "source": customer()});
        v[field] = val.clone();
        let (_, r) = check(v);
        assert!(
            r.errors.iter().any(|e| e.contains("money")),
            "{field}={val}: {:?}",
            r.errors
        );
    }
    let (_, r) = check(
        json!({"kind": "bar", "title": "t", "categories": ["JPY 5,000,000"], "series": [{"name": "s", "values": [1]}], "source": customer()}),
    );
    assert!(!r.ok());
    // A word containing "jpy" is not money.
    let (_, r) = check(
        json!({"kind": "bar", "title": "Corporate sites", "categories": ["Warp gate"], "series": [{"name": "s", "values": [1]}], "source": customer()}),
    );
    assert!(r.ok(), "{:?}", r.errors);
}

#[test]
fn guard_refuses_accuracy() {
    let (_, r) = check(
        json!({"kind": "bar", "title": "Matching accuracy", "unit": "%", "categories": ["Invoices"], "series": [{"name": "s", "values": [98]}], "source": customer()}),
    );
    assert!(
        r.errors.iter().any(|e| e.contains("accuracy")),
        "{:?}",
        r.errors
    );
    let (_, r) = check(
        json!({"kind": "architecture", "title": "t", "nodes": [{"id": "a", "label": "99% accuracy engine", "icon": "server"}], "edges": []}),
    );
    assert!(!r.ok());
}

#[test]
fn guard_sources() {
    let base = json!({"kind": "bar", "title": "t", "categories": ["a"], "series": [{"name": "s", "values": [1]}]});
    let mut v = base.clone();
    v["source"] = Value::Null;
    assert!(!check(v).1.ok());
    let mut v = base.clone();
    v["source"] = json!({"type": "customer", "note": "  "});
    assert!(!check(v).1.ok());
    let mut v = base.clone();
    v["source"] = json!({"type": "analysis", "note": "", "from": []});
    assert!(!check(v).1.ok());
    // an unknown source kind does not parse
    let mut v = base.clone();
    v["source"] = json!({"type": "calc", "calc": "storage", "inputs": {}});
    assert!(Spec::from_value(v).is_err());
    let mut v = base.clone();
    v["source"] = json!({"type": "erp", "ref": "requirements.unknown"});
    assert!(check(v)
        .1
        .errors
        .iter()
        .any(|e| e.contains("could not be resolved")));
    // ERP numbers replace what the model typed.
    let (s, r) = check(
        json!({"kind": "gantt", "title": "t", "source": {"type": "erp", "ref": "proposal.phases"},
        "tasks": [{"name": "Invented", "start": 0, "length": 40}], "categories": [], "series": []}),
    );
    assert!(r.ok(), "{:?}", r.errors);
    let Spec::Chart(c) = s else { panic!() };
    assert_eq!(c.tasks.len(), 1);
    assert_eq!(c.tasks[0].name, "Survey");
}

#[test]
fn guard_ranges() {
    let (_, r) = check(
        json!({"kind": "bar", "title": "t", "categories": ["a", "b"], "series": [{"name": "s", "values": [1]}], "source": customer()}),
    );
    assert!(r
        .errors
        .iter()
        .any(|e| e.contains("1 values for 2 categories")));
    let (_, r) = check(
        json!({"kind": "donut", "title": "t", "categories": ["a", "b"], "series": [{"name": "s", "values": [1, -2]}], "source": customer()}),
    );
    assert!(r.errors.iter().any(|e| e.contains("negative")));
    let (_, r) = check(
        json!({"kind": "gantt", "title": "t", "tasks": [{"name": "x", "start": -1, "length": 2}], "categories": [], "series": [], "source": customer()}),
    );
    assert!(!r.ok());
    let (_, r) = check(
        json!({"kind": "gantt", "title": "t", "tasks": [{"name": "x", "start": 0, "length": 2}],
        "milestones": [{"name": "late", "at": 9}], "categories": [], "series": [], "source": customer()}),
    );
    assert!(r.errors.iter().any(|e| e.contains("outside the timeline")));
    let (_, r) = check(
        json!({"kind": "radar", "title": "t", "categories": ["a", "b"], "series": [{"name": "s", "values": [1, 2]}], "source": customer()}),
    );
    assert!(!r.ok());
    let (_, r) = check(
        json!({"kind": "line", "title": "t", "categories": ["a"], "series": [{"name": "s", "values": [1]}],
        "annotations": [{"type": "highlight", "series": "nope", "index": 0}], "source": customer()}),
    );
    assert!(!r.ok());
}

#[test]
fn guard_diagrams() {
    let ok = |nodes: Value, edges: Value, kind: &str| {
        check(
            json!({"kind": kind, "title": "t", "groups": [{"id": "g", "label": "Site", "style": "site"}], "nodes": nodes, "edges": edges}),
        )
    };
    // Edge to a node that does not exist.
    let (_, r) = ok(
        json!([{"id": "a", "label": "A", "icon": "server"}]),
        json!([{"from": "a", "to": "zz", "kind": "data"}]),
        "architecture",
    );
    assert!(r.errors.iter().any(|e| e.contains("does not exist")));
    // Unknown icon, bad group.
    let (_, r) = ok(
        json!([{"id": "a", "label": "A", "icon": "rocket", "group": "nope"}]),
        json!([]),
        "architecture",
    );
    assert_eq!(r.errors.len(), 2, "{:?}", r.errors);
    // Cycle in a process.
    let (_, r) = ok(
        json!([{"id": "a", "label": "A", "icon": "user"}, {"id": "b", "label": "B", "icon": "user"}]),
        json!([{"from": "a", "to": "b", "kind": "data"}, {"from": "b", "to": "a", "kind": "data"}]),
        "process",
    );
    assert!(r.errors.iter().any(|e| e.contains("loop")));
    // The same cycle is fine in an architecture diagram.
    let (_, r) = ok(
        json!([{"id": "a", "label": "A", "icon": "user"}, {"id": "b", "label": "B", "icon": "user"}]),
        json!([{"from": "a", "to": "b", "kind": "data"}, {"from": "b", "to": "a", "kind": "data"}]),
        "architecture",
    );
    assert!(r.ok(), "{:?}", r.errors);
    // sku must exist and the label must be the product's name.
    let (_, r) = ok(
        json!([{"id": "a", "label": "Super Box", "icon": "server", "sku": "EGW-1"}]),
        json!([]),
        "architecture",
    );
    assert!(r
        .errors
        .iter()
        .any(|e| e.contains("must be the product's name")));
    let (_, r) = ok(
        json!([{"id": "a", "label": "Edge Gateway", "icon": "server", "sku": "NOPE"}]),
        json!([]),
        "architecture",
    );
    assert!(r.errors.iter().any(|e| e.contains("not in the catalog")));
    // A node without a sku is a plain node.
    let (_, r) = ok(
        json!([{"id": "a", "label": "File server", "icon": "server"}]),
        json!([]),
        "architecture",
    );
    assert!(r.ok(), "{:?}", r.errors);
    // A label naming a catalog product gets its sku.
    let (s, r) = ok(
        json!([{"id": "a", "label": "edge  gateway", "icon": "server"}]),
        json!([]),
        "architecture",
    );
    assert!(r.ok(), "{:?}", r.errors);
    let Spec::Diagram(d) = s else { panic!() };
    assert_eq!(d.nodes[0].sku.as_deref(), Some("EGW-1"));
    let (_, r) = ok(
        json!([{"id": "a", "label": "Go live", "icon": "user"}]),
        json!([]),
        "process",
    );
    assert!(r.ok(), "{:?}", r.errors);
}

#[test]
fn guard_cleans_em_dashes() {
    let (s, r) = check(
        json!({"kind": "bar", "title": "Sites \u{2014} users", "categories": ["a"], "series": [{"name": "s", "values": [1]}], "source": customer()}),
    );
    assert!(r.ok());
    assert!(!s.title().contains('\u{2014}'));
}

#[test]
fn requirement_answers_become_numbers() {
    let d = requirement_numbers("Site A: 4; Site B: 6").unwrap();
    assert_eq!(d.categories, vec!["Site A", "Site B"]);
    assert_eq!(d.series[0].values, vec![4.0, 6.0]);
    let d = requirement_numbers("40 users").unwrap();
    assert_eq!(d.series[0].values, vec![40.0]);
    assert!(requirement_numbers("about forty, maybe more").is_none());
}

// ---------------------------------------------------------------------------------------------------------
// Custom SVG sanitiser
// ---------------------------------------------------------------------------------------------------------

#[test]
fn sanitizer_strips_dangerous_content() {
    let dirty = r##"<?xml version="1.0"?><!DOCTYPE svg><svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="50" onload="alert(1)">
<script>alert(2)</script>
<style>@import url(http://evil/x.css); rect { fill: url(http://evil/a) }</style>
<foreignObject><div xmlns="http://www.w3.org/1999/xhtml">hi</div></foreignObject>
<image href="http://evil/x.png" width="10" height="10"/>
<a href="javascript:alert(3)"><text x="1" y="1">link</text></a>
<rect x="0" y="0" width="10" height="10" fill="url(http://evil/p)" style="fill:red" class="c" onclick="x()"/>
<rect x="0" y="0" width="10" height="10" fill="url(#g)"/>
<use href="#g"/><use xlink:href="http://evil/s.svg#a"/><use href="data:image/svg+xml;base64,AAA"/>
<text x="5" y="20">Hello &amp; welcome</text>
</svg>"##;
    let out = sanitize::sanitize(dirty, 300).unwrap();
    let s = &out.svg;
    for bad in [
        "script",
        "alert",
        "@import",
        "foreignObject",
        "<image",
        "javascript",
        "evil",
        "style=",
        "class=",
        "onclick",
        "onload",
        "<a",
        "data:",
    ] {
        assert!(!s.contains(bad), "{bad} survived: {s}");
    }
    assert!(s.contains(r#"viewBox="0 0 100 50""#), "viewBox forced: {s}");
    assert!(s.contains(r##"fill="url(#g)""##));
    assert!(s.contains(r##"<use href="#g"/>"##));
    assert!(s.contains("Hello &amp; welcome"));
    assert_eq!(out.texts, vec!["Hello & welcome".to_string()]);
    assert!(!out.removed.is_empty());
    // It still rasterises.
    assert!(visuals::raster::svg_to_png(s, 200).is_ok());
}

#[test]
fn sanitizer_refuses() {
    let huge = format!(
        r#"<svg viewBox="0 0 10 10">{}</svg>"#,
        "<rect width=\"1\" height=\"1\"/>".repeat(20_000)
    );
    assert!(sanitize::sanitize(&huge, 300)
        .unwrap_err()
        .to_string()
        .contains("KB"));
    let many = format!(r#"<svg viewBox="0 0 10 10">{}</svg>"#, "<g/>".repeat(5000));
    assert!(sanitize::sanitize(&many, 300)
        .unwrap_err()
        .to_string()
        .contains("elements"));
    assert!(sanitize::sanitize("<html><body/></html>", 300).is_err());
    assert!(sanitize::sanitize(r#"<svg><rect/></svg>"#, 300)
        .unwrap_err()
        .to_string()
        .contains("viewBox"));
    assert!(sanitize::sanitize(r#"<svg viewBox="0 0 1 1"><rect>"#, 300).is_err());
    assert!(sanitize::sanitize(r#"<svg viewBox="0 0 1 1"><text>&xxe;</text></svg>"#, 300).is_err());
}

#[test]
fn custom_svg_runs_text_guards() {
    let mk = |t: &str| json!({"kind": "custom_svg", "title": "x", "svg": format!(r#"<svg viewBox="0 0 10 10"><text x="1" y="5">{t}</text></svg>"#), "source": null});
    let (_, r) = check(mk("Price: JPY 50,000"));
    assert!(r.errors.iter().any(|e| e.contains("money")));
    let (_, r) = check(mk("98% accuracy"));
    assert!(!r.ok());
    let (_, r) = check(mk("Edge Gateway at the office"));
    assert!(r.ok(), "{:?}", r.errors);
    // The guard stores the sanitised drawing.
    let (s, r) = check(
        json!({"kind": "custom_svg", "title": "x", "svg": r#"<svg viewBox="0 0 10 10"><script>x</script><circle r="2"/></svg>"#, "source": null}),
    );
    assert!(r.ok());
    assert!(!s.to_value()["svg"].as_str().unwrap().contains("script"));
    let small = GuardContext {
        custom_svg_max_kb: 1,
        ..ctx()
    };
    let mut big = spec(
        json!({"kind": "custom_svg", "title": "x", "svg": format!(r#"<svg viewBox="0 0 10 10">{}</svg>"#, "<g/>".repeat(400)), "source": null}),
    );
    assert!(!guard(&mut big, &small).ok());
}
