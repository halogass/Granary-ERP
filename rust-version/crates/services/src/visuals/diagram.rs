//! Diagrams: architecture and network (layered auto layout, groups as framed regions, count badges) and
//! process (numbered steps left to right or top to bottom, groups as swimlanes).
//!
//! Layout: layers by longest path over the links (cycles broken in node order); each group is a band
//! across the layers (bands never overlap, so frames never overlap); nodes of a layer inside a band are
//! ordered by a few barycenter sweeps to cut crossings. Ungrouped nodes in layers with no framed node
//! are centred over the whole drawing. Links are curves between ports spread along the node sides,
//! styled by link kind and explained in a legend; link labels sit on white pills placed away from nodes.

use std::collections::BTreeMap;
use std::fmt::Write;

use super::i18n;
use super::icons;
use super::spec::*;
use super::svg::{esc, n, Anchor, Doc, TextStyle};
use super::text::{self, truncate, width, wrap};
use super::theme;
use super::{RenderOptions, Result, VisualError};

#[derive(Clone, Copy)]
struct EdgeStyle {
    color: &'static str,
    width: f64,
    dash: &'static str,
    arrow: bool,
}

fn edge_style(k: EdgeKind, process: bool) -> EdgeStyle {
    let s = match k {
        EdgeKind::Ethernet => EdgeStyle {
            color: theme::SLATE,
            width: 1.6,
            dash: "",
            arrow: false,
        },
        EdgeKind::Poe => EdgeStyle {
            color: theme::BLUE,
            width: 2.0,
            dash: "",
            arrow: false,
        },
        EdgeKind::Fiber => EdgeStyle {
            color: theme::GREEN,
            width: 2.0,
            dash: "",
            arrow: false,
        },
        EdgeKind::FourG => EdgeStyle {
            color: theme::BLUE,
            width: 1.6,
            dash: "6 4",
            arrow: false,
        },
        EdgeKind::Wifi => EdgeStyle {
            color: theme::BLUE,
            width: 1.6,
            dash: "1.5 3.5",
            arrow: false,
        },
        EdgeKind::Power => EdgeStyle {
            color: theme::AMBER,
            width: 2.0,
            dash: "",
            arrow: false,
        },
        EdgeKind::Data => EdgeStyle {
            color: theme::INK,
            width: 1.4,
            dash: "",
            arrow: true,
        },
        EdgeKind::Api => EdgeStyle {
            color: theme::MAROON,
            width: 1.6,
            dash: "5 3",
            arrow: true,
        },
        EdgeKind::Relay => EdgeStyle {
            color: theme::SLATE,
            width: 1.6,
            dash: "8 3 2 3",
            arrow: true,
        },
    };
    if process {
        EdgeStyle { arrow: true, ..s }
    } else {
        s
    }
}

fn marker_id(color: &str) -> String {
    format!("ah{}", color.trim_start_matches('#'))
}

#[derive(Clone, Debug)]
struct Placed {
    /// main axis start, cross axis start (screen coordinates after mapping)
    x: f64,
    y: f64,
}

/// Layers by longest path over forward links (links that close a cycle in node order are ignored).
fn layers(nodes: &[Node], edges: &[Edge]) -> (Vec<usize>, Vec<(usize, usize)>) {
    let idx: BTreeMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let mut adj = vec![Vec::new(); nodes.len()];
    let mut pairs = Vec::new();
    for e in edges {
        if let (Some(&a), Some(&b)) = (idx.get(e.from.as_str()), idx.get(e.to.as_str())) {
            if a != b {
                adj[a].push(b);
                pairs.push((a, b));
            }
        }
    }
    // Back edges by DFS.
    let mut state = vec![0u8; nodes.len()];
    let mut back: Vec<(usize, usize)> = Vec::new();
    for s in 0..nodes.len() {
        if state[s] != 0 {
            continue;
        }
        let mut stack = vec![(s, 0usize)];
        state[s] = 1;
        while let Some((v, i)) = stack.pop() {
            if i < adj[v].len() {
                stack.push((v, i + 1));
                let w = adj[v][i];
                if state[w] == 1 {
                    back.push((v, w));
                } else if state[w] == 0 {
                    state[w] = 1;
                    stack.push((w, 0));
                }
            } else {
                state[v] = 2;
            }
        }
    }
    let fwd: Vec<(usize, usize)> = pairs
        .iter()
        .copied()
        .filter(|p| !back.contains(p))
        .collect();
    // Longest path (Kahn).
    let mut indeg = vec![0usize; nodes.len()];
    for (_, b) in &fwd {
        indeg[*b] += 1;
    }
    let mut layer = vec![0usize; nodes.len()];
    let mut queue: Vec<usize> = (0..nodes.len()).filter(|&i| indeg[i] == 0).collect();
    let mut qi = 0;
    while qi < queue.len() {
        let v = queue[qi];
        qi += 1;
        for (a, b) in &fwd {
            if *a == v {
                layer[*b] = layer[*b].max(layer[v] + 1);
                indeg[*b] -= 1;
                if indeg[*b] == 0 {
                    queue.push(*b);
                }
            }
        }
    }
    // Pull sources next to their first target (a power feed does not need to sit at the far left).
    for v in 0..nodes.len() {
        if fwd.iter().any(|(_, b)| *b == v) {
            continue;
        }
        let targets: Vec<usize> = fwd
            .iter()
            .filter(|(a, _)| *a == v)
            .map(|(_, b)| layer[*b])
            .collect();
        if let Some(&min_t) = targets.iter().min() {
            if min_t > 0 {
                layer[v] = min_t - 1;
            }
        }
    }
    (layer, pairs)
}

struct Geo {
    w: f64,
    h: f64,
    icon_left: bool,
    icon: f64,
    text_w: f64,
}

pub fn render(spec: &DiagramSpec, opts: RenderOptions) -> Result<String> {
    if spec.nodes.is_empty() {
        return Err(VisualError::Invalid("the diagram has no nodes".into()));
    }
    let m = theme::metrics(opts.size);
    let process = spec.kind == DiagramKind::Process;
    let mut d = Doc::new(m.width);
    let mut y = m.pad;
    let lang = spec.language;
    if !spec.title.trim().is_empty() {
        let lines = wrap(spec.title.trim(), m.width - 2.0 * m.pad, m.title, true, 2);
        let lh = m.title * 1.3;
        d.lines(
            m.pad,
            y + m.title,
            &lines,
            lh,
            TextStyle::new(m.title, theme::INK).bold(),
        );
        y += lines.len() as f64 * lh + 10.0;
    }

    let nodes = &spec.nodes;
    let (layer, pairs) = layers(nodes, &spec.edges);
    let nlayers = layer.iter().max().map_or(1, |m| m + 1);

    // Bands: groups in spec order that have nodes, then ungrouped.
    let gidx: BTreeMap<&str, usize> = spec
        .groups
        .iter()
        .enumerate()
        .map(|(i, g)| (g.id.as_str(), i))
        .collect();
    let mut band_groups: Vec<Option<usize>> = Vec::new();
    for (gi, g) in spec.groups.iter().enumerate() {
        if nodes
            .iter()
            .any(|n| n.group.as_deref() == Some(g.id.as_str()))
        {
            band_groups.push(Some(gi));
        }
    }
    let has_free = nodes
        .iter()
        .any(|n| n.group.as_deref().and_then(|g| gidx.get(g)).is_none());
    if has_free {
        band_groups.push(None);
    }
    let band_of: Vec<usize> = nodes
        .iter()
        .map(|n| {
            let g = n.group.as_deref().and_then(|g| gidx.get(g)).copied();
            band_groups.iter().position(|b| *b == g).unwrap_or(0)
        })
        .collect();
    let framed = |b: usize| band_groups[b].is_some();
    let lanes = process && band_groups.iter().any(|b| b.is_some());

    // Layers with no framed node: ungrouped nodes there float over the whole cross range.
    let framed_layer: Vec<bool> = (0..nlayers)
        .map(|l| (0..nodes.len()).any(|i| layer[i] == l && framed(band_of[i])))
        .collect();
    let floating: Vec<bool> = (0..nodes.len())
        .map(|i| !lanes && !framed(band_of[i]) && !framed_layer[layer[i]])
        .collect();

    // Cells and barycenter ordering.
    let mut order: Vec<f64> = (0..nodes.len()).map(|i| i as f64).collect();
    for sweep in 0..6 {
        let forward = sweep % 2 == 0;
        let mut rank = vec![0.0; nodes.len()];
        // global cross rank = band * 1000 + position in cell
        let mut cells: BTreeMap<(usize, usize, bool), Vec<usize>> = BTreeMap::new();
        for i in 0..nodes.len() {
            cells
                .entry((layer[i], band_of[i], floating[i]))
                .or_default()
                .push(i);
        }
        for v in cells.values_mut() {
            v.sort_by(|a, b| order[*a].partial_cmp(&order[*b]).unwrap());
            for (k, &i) in v.iter().enumerate() {
                rank[i] = band_of[i] as f64 * 1000.0 + k as f64;
            }
        }
        let mut new_order = order.clone();
        for i in 0..nodes.len() {
            let nb: Vec<f64> = pairs
                .iter()
                .filter_map(|(a, b)| {
                    if forward && *b == i && layer[*a] < layer[i] {
                        Some(rank[*a])
                    } else if !forward && *a == i && layer[*b] > layer[i] {
                        Some(rank[*b])
                    } else {
                        None
                    }
                })
                .collect();
            if !nb.is_empty() {
                new_order[i] = nb.iter().sum::<f64>() / nb.len() as f64 + i as f64 * 1e-6;
            }
        }
        order = new_order;
    }
    let mut cells: BTreeMap<(usize, usize, bool), Vec<usize>> = BTreeMap::new();
    for i in 0..nodes.len() {
        cells
            .entry((layer[i], band_of[i], floating[i]))
            .or_default()
            .push(i);
    }
    for v in cells.values_mut() {
        v.sort_by(|a, b| order[*a].partial_cmp(&order[*b]).unwrap().then(a.cmp(b)));
    }

    // Direction: LR unless asked otherwise or the layers do not fit across.
    let inner = m.width - 2.0 * m.pad;
    let fp = 12.0; // frame padding
    let label_h = m.small * 1.4 + 6.0; // frame label
    let lane_head = if lanes {
        (inner * 0.15).clamp(56.0, 90.0)
    } else {
        0.0
    };
    // Process steps carry short or no link labels: tighter gaps keep more flows left to right.
    let main_gap: f64 = match (process, m.width < 500.0) {
        (true, true) => 26.0,
        (true, false) => 30.0,
        (false, true) => 38.0,
        (false, false) => 54.0,
    };
    let cross_gap = 16.0;
    let min_w = if process {
        80.0
    } else if m.width < 500.0 {
        84.0
    } else {
        96.0
    };
    let max_w = 170.0;
    let any_frame = band_groups.iter().any(|b| b.is_some());
    // Left to right, a link label sits in the gap between columns; between two frames it must also
    // clear both borders. Widen the gap for the longest label (up to a cap) when there is room.
    let label_need = spec
        .edges
        .iter()
        .filter_map(|e| e.label.as_deref().map(str::trim).filter(|l| !l.is_empty()))
        .map(|l| width(&truncate(l, 120.0, m.tiny, false), m.tiny, false) + 8.0)
        .fold(0.0, f64::max);
    let lr_gap = if label_need > 0.0 && !process {
        let extra = if any_frame { 2.0 * fp + 12.0 } else { 16.0 };
        main_gap.max((label_need + extra).min(140.0))
    } else {
        main_gap
    };
    let fit =
        |gap: f64| (inner - lane_head - 2.0 * fp - (nlayers as f64 - 1.0) * gap) / nlayers as f64;
    // The widest gap that still leaves nodes a little under their usual minimum (labels matter more).
    let lr_min_w = if lr_gap > main_gap {
        min_w - 8.0
    } else {
        min_w
    };
    let gap_max = if nlayers > 1 {
        (inner - lane_head - 2.0 * fp - nlayers as f64 * lr_min_w) / (nlayers as f64 - 1.0)
    } else {
        lr_gap
    };
    let lr_gap = lr_gap.min(gap_max).max(main_gap);
    let lr_fit_w = fit(lr_gap);
    let min_w = if lr_gap > main_gap { lr_min_w } else { min_w };
    let lr = spec.direction == Direction::LR && lr_fit_w >= min_w;
    let main_gap = if lr {
        lr_gap
    } else if any_frame || lanes {
        // Top to bottom, a frame's label sits in the gap above its first row: leave room for it, and
        // for a link label between two frames.
        let link_room = if label_need > 0.0 {
            m.tiny + 6.0 + 10.0
        } else {
            16.0
        };
        main_gap.max(2.0 * fp + label_h + link_room)
    } else {
        main_gap
    };

    // Cross counts per band.
    let band_cross_count: Vec<usize> = (0..band_groups.len())
        .map(|b| {
            cells
                .iter()
                .filter(|((_, bb, fl), _)| *bb == b && !fl)
                .map(|(_, v)| v.len())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let float_count = cells
        .iter()
        .filter(|((_, _, fl), _)| *fl)
        .map(|(_, v)| v.len())
        .max()
        .unwrap_or(0);
    // Tracks: bands whose layer ranges do not overlap share the same cross range (side by side along
    // the flow), so a site frame and the customer systems after it sit next to each other. Lanes never
    // share: each lane spans the whole flow.
    let band_range: Vec<Option<(usize, usize)>> = (0..band_groups.len())
        .map(|b| {
            let ls: Vec<usize> = (0..nodes.len())
                .filter(|&i| band_of[i] == b && !floating[i])
                .map(|i| layer[i])
                .collect();
            Some((*ls.iter().min()?, *ls.iter().max()?))
        })
        .collect();
    let mut tracks: Vec<Vec<usize>> = Vec::new();
    let mut track_of = vec![usize::MAX; band_groups.len()];
    for b in 0..band_groups.len() {
        let Some((lo, hi)) = band_range[b] else {
            continue;
        };
        let slot = if lanes {
            None
        } else {
            tracks.iter().position(|t| {
                t.iter()
                    .all(|&o| band_range[o].is_none_or(|(olo, ohi)| hi < olo || lo > ohi))
            })
        };
        match slot {
            Some(t) => {
                tracks[t].push(b);
                track_of[b] = t;
            }
            None => {
                tracks.push(vec![b]);
                track_of[b] = tracks.len() - 1;
            }
        }
    }

    // Node width.
    let node_w = if lr {
        lr_fit_w.min(max_w)
    } else {
        // TB: tracks side by side across the width.
        let track_cols: Vec<usize> = tracks
            .iter()
            .map(|t| t.iter().map(|&b| band_cross_count[b]).max().unwrap_or(0))
            .collect();
        let total_cols: usize = track_cols.iter().sum::<usize>().max(float_count).max(1);
        let framed_tracks = tracks
            .iter()
            .filter(|t| t.iter().any(|&b| framed(b) || lanes))
            .count() as f64;
        let avail = inner
            - lane_head
            - framed_tracks * 2.0 * fp
            - (tracks.len() as f64 - 1.0).max(0.0) * 14.0
            - (total_cols as f64 - 1.0) * cross_gap;
        (avail / total_cols as f64).clamp(min_w.min(80.0), max_w)
    };
    let icon_left = node_w >= 150.0;
    let icon = if icon_left { 28.0 } else { 24.0 };
    let pad_in = 10.0;
    let text_w = if icon_left {
        node_w - icon - 3.0 * pad_in
    } else {
        node_w - 12.0
    };
    let size = m.small;
    let lh = size * 1.22;
    let max_label_lines = if process && !icon_left { 4 } else { 3 };
    let node_lines: Vec<Vec<String>> = nodes
        .iter()
        .map(|nd| {
            wrap(
                nd.label.trim(),
                text_w,
                size,
                nd.sku.is_some(),
                max_label_lines,
            )
        })
        .collect();
    let max_lines = node_lines.iter().map(|l| l.len()).max().unwrap_or(1).max(1) as f64;
    let node_h = if icon_left {
        (max_lines * lh).max(icon) + 2.0 * pad_in + 2.0
    } else {
        pad_in + icon + 6.0 + max_lines * lh + pad_in - 2.0
    };
    let geo = Geo {
        w: node_w,
        h: node_h,
        icon_left,
        icon,
        text_w,
    };
    let (main_ext, cross_ext) = if lr { (geo.w, geo.h) } else { (geo.h, geo.w) };

    // Band cross extents, placed track by track (bands in one track centred on it).
    let band_gap = 14.0;
    // Left to right, a frame label that does not fit the frame's width wraps to a second line.
    let flh = size * 1.25;
    let band_label_lines: Vec<Vec<String>> = (0..band_groups.len())
        .map(|b| match (band_groups[b], band_range[b]) {
            (Some(gi), Some((lo, hi))) if lr && !lanes => {
                let g = &spec.groups[gi];
                let fw = (hi - lo + 1) as f64 * main_ext + (hi - lo) as f64 * main_gap + 2.0 * fp;
                let icon_w = if g.style == GroupStyle::Cloud {
                    18.0
                } else {
                    0.0
                };
                wrap(&g.label, fw - 18.0 - icon_w, size, true, 2)
            }
            _ => vec![],
        })
        .collect();
    let band_label_h = |b: usize| label_h + (band_label_lines[b].len().max(1) as f64 - 1.0) * flh;
    let frame_cross_extra = |b: usize| -> f64 {
        if lanes {
            2.0 * fp
        } else if framed(b) {
            if lr {
                2.0 * fp + band_label_h(b)
            } else {
                2.0 * fp
            }
        } else {
            0.0
        }
    };
    let band_ext: Vec<f64> = (0..band_groups.len())
        .map(|b| {
            let c = band_cross_count[b];
            if c == 0 {
                0.0
            } else {
                c as f64 * cross_ext + (c as f64 - 1.0) * cross_gap + frame_cross_extra(b)
            }
        })
        .collect();
    let mut band_start = vec![0.0; band_groups.len()];
    let mut acc = 0.0;
    for t in &tracks {
        let ext = t.iter().map(|&b| band_ext[b]).fold(0.0, f64::max);
        for &b in t {
            band_start[b] = acc + (ext - band_ext[b]) / 2.0;
        }
        if ext > 0.0 {
            acc += ext + band_gap;
        }
    }
    let _ = &track_of;
    let mut cross_total = (acc - band_gap).max(0.0);
    let float_ext = if float_count > 0 {
        float_count as f64 * cross_ext + (float_count as f64 - 1.0) * cross_gap
    } else {
        0.0
    };
    cross_total = cross_total.max(float_ext);

    // Origins. LR: main = x, cross = y. TB: main = y, cross = x.
    let top = y + 4.0;
    let (main0, cross0) = if lr {
        (m.pad + lane_head + fp, top)
    } else {
        let head = if lanes {
            label_h + 6.0
        } else if any_frame {
            label_h
        } else {
            0.0
        };
        (top + head + fp, m.pad + lane_head)
    };
    // TB: centre the bands across the width when they are narrower.
    let cross0 = if !lr {
        cross0 + ((inner - lane_head) - cross_total).max(0.0) / 2.0
    } else {
        cross0
    };
    let main_of = |l: usize| main0 + l as f64 * (main_ext + main_gap);
    let mut placed: Vec<Placed> = vec![Placed { x: 0.0, y: 0.0 }; nodes.len()];
    for ((l, b, fl), v) in &cells {
        let k = v.len() as f64;
        let span = k * cross_ext + (k - 1.0) * cross_gap;
        let (c_lo, c_ext) = if *fl {
            (cross0, cross_total)
        } else {
            let inner_off = if framed(*b) || lanes {
                fp + if lr && !lanes { band_label_h(*b) } else { 0.0 }
            } else {
                0.0
            };
            (
                cross0 + band_start[*b] + inner_off,
                band_ext[*b] - frame_cross_extra(*b),
            )
        };
        let start = c_lo + (c_ext - span) / 2.0;
        for (j, &i) in v.iter().enumerate() {
            let c = start + j as f64 * (cross_ext + cross_gap);
            let mpos = main_of(*l);
            let (x, yy) = if lr { (mpos, c) } else { (c, mpos) };
            placed[i] = Placed { x, y: yy };
        }
    }

    // Frames.
    let main_end = main_of(nlayers - 1) + main_ext;
    let mut frames_svg = String::new();
    let mut frame_labels: Vec<(f64, f64, f64, f64)> = Vec::new();
    let mut frame_rects: Vec<(f64, f64, f64, f64)> = Vec::new();
    for (b, g) in band_groups.iter().enumerate() {
        if band_ext[b] <= 0.0 {
            continue;
        }
        let members: Vec<usize> = (0..nodes.len())
            .filter(|&i| band_of[i] == b && !floating[i])
            .collect();
        if members.is_empty() {
            continue;
        }
        let (style, label) = match g {
            Some(gi) => (
                if lanes {
                    GroupStyle::Lane
                } else {
                    spec.groups[*gi].style
                },
                spec.groups[*gi].label.as_str(),
            ),
            None if lanes => (GroupStyle::Lane, ""),
            None => continue,
        };
        let c_a = cross0 + band_start[b];
        let c_b = c_a + band_ext[b];
        let (m_a, m_b) = if lanes {
            (main0 - fp, main_end + fp)
        } else {
            let lmin = members.iter().map(|&i| layer[i]).min().unwrap_or(0);
            let lmax = members.iter().map(|&i| layer[i]).max().unwrap_or(0);
            (main_of(lmin) - fp, main_of(lmax) + main_ext + fp)
        };
        let (x0, y0, x1, y1) = if lr {
            (m_a, c_a, m_b, c_b)
        } else {
            let head = if lanes { label_h + 6.0 } else { label_h };
            (c_a, m_a - head, c_b, m_b)
        };
        frame_rects.push(if lanes && lr {
            (m.pad, y0, x1, y1)
        } else {
            (x0, y0, x1, y1)
        });
        let (stroke, fill, dash) = match style {
            GroupStyle::Site => (theme::SLATE, "none", "5 4"),
            GroupStyle::CustomerSystem => (theme::BLUE, theme::BLUE_TINT, ""),
            GroupStyle::Cloud => (theme::BLUE, "none", "2 4"),
            GroupStyle::Zone => (theme::AMBER, theme::AMBER_TINT, "5 4"),
            GroupStyle::Lane => (theme::AXIS, theme::SLATE_TINT, ""),
        };
        if lanes {
            // Lane: full width band with a header column (LR) or header row (TB).
            let _ = write!(
                frames_svg,
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{fill}" stroke="{stroke}" stroke-width="1"/>"#,
                n(if lr { m.pad } else { x0 }),
                n(y0),
                n(if lr { x1 - m.pad } else { x1 - x0 }),
                n(y1 - y0)
            );
            if lr {
                let lines = wrap(label, lane_head - 16.0, size, true, 3);
                let ty = (y0 + y1) / 2.0 - (lines.len() as f64 - 1.0) * lh / 2.0 + size * 0.35;
                let _ = write!(
                    frames_svg,
                    r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="1"/>"#,
                    n(m.pad + lane_head),
                    n(y0),
                    n(m.pad + lane_head),
                    n(y1),
                    theme::AXIS
                );
                for (k, l) in lines.iter().enumerate() {
                    let _ = write!(
                        frames_svg,
                        r#"<text x="{}" y="{}" font-size="{}" font-weight="600" fill="{}">{}</text>"#,
                        n(m.pad + 8.0),
                        n(ty + k as f64 * lh),
                        n(size),
                        theme::INK,
                        esc(l)
                    );
                }
            } else {
                let t = truncate(label, x1 - x0 - 12.0, size, true);
                let _ = write!(
                    frames_svg,
                    r#"<text x="{}" y="{}" font-size="{}" font-weight="600" fill="{}" text-anchor="middle">{}</text>"#,
                    n((x0 + x1) / 2.0),
                    n(y0 + size + 5.0),
                    n(size),
                    theme::INK,
                    esc(&t)
                );
            }
            continue;
        }
        let _ = write!(
            frames_svg,
            r#"<rect x="{}" y="{}" width="{}" height="{}" rx="10" fill="{fill}" stroke="{stroke}" stroke-width="1.3"{}/>"#,
            n(x0),
            n(y0),
            n(x1 - x0),
            n(y1 - y0),
            if dash.is_empty() {
                String::new()
            } else {
                format!(r#" stroke-dasharray="{dash}""#)
            }
        );
        let mut lx = x0 + 10.0;
        if style == GroupStyle::Cloud {
            frames_svg.push_str(&icons::draw(
                "cloud",
                lx - 2.0,
                y0 + 4.0,
                16.0,
                Some(theme::BLUE),
            ));
            lx += 18.0;
        }
        let lines = if lr && !band_label_lines[b].is_empty() {
            band_label_lines[b].clone()
        } else {
            vec![truncate(label, x1 - lx - 8.0, size, true)]
        };
        let lw = lines
            .iter()
            .map(|l| width(l, size, true))
            .fold(0.0, f64::max);
        frame_labels.push((
            lx - 2.0,
            y0 + 2.0,
            lx + lw + 2.0,
            y0 + size + 9.0 + (lines.len() as f64 - 1.0) * flh,
        ));
        for (k, t) in lines.iter().enumerate() {
            let _ = write!(
                frames_svg,
                r#"<text x="{}" y="{}" font-size="{}" font-weight="600" fill="{stroke}">{}</text>"#,
                n(lx),
                n(y0 + size + 5.0 + k as f64 * flh),
                n(size),
                esc(t)
            );
        }
    }
    d.raw(&frames_svg);

    // Ports: spread link ends along the node sides.
    let rect_of = |i: usize| {
        (
            placed[i].x,
            placed[i].y,
            placed[i].x + geo.w,
            placed[i].y + geo.h,
        )
    };
    #[derive(Clone, Copy, PartialEq)]
    enum Side {
        MainEnd,
        MainStart,
        CrossEnd,
    }
    let mut ends: Vec<(usize, Side, usize, f64, usize)> = Vec::new(); // (node, side, edge index, other end pos, 0 from / 1 to)
    let mut edge_sides: Vec<(Side, Side)> = Vec::new();
    let idx: BTreeMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let valid_edges: Vec<(usize, usize, &Edge)> = spec
        .edges
        .iter()
        .filter_map(
            |e| match (idx.get(e.from.as_str()), idx.get(e.to.as_str())) {
                (Some(&a), Some(&b)) if a != b => Some((a, b, e)),
                _ => None,
            },
        )
        .collect();
    let cross_c = |i: usize| {
        if lr {
            placed[i].y + geo.h / 2.0
        } else {
            placed[i].x + geo.w / 2.0
        }
    };
    let main_c = |i: usize| {
        if lr {
            placed[i].x + geo.w / 2.0
        } else {
            placed[i].y + geo.h / 2.0
        }
    };
    for (k, (a, b, _)) in valid_edges.iter().enumerate() {
        let sides = if layer[*a] < layer[*b] {
            (Side::MainEnd, Side::MainStart)
        } else if layer[*a] > layer[*b] {
            (Side::MainStart, Side::MainEnd)
        } else {
            (Side::CrossEnd, Side::CrossEnd)
        };
        edge_sides.push(sides);
        let other_a = if sides.0 == Side::CrossEnd {
            main_c(*b)
        } else {
            cross_c(*b)
        };
        let other_b = if sides.1 == Side::CrossEnd {
            main_c(*a)
        } else {
            cross_c(*a)
        };
        ends.push((*a, sides.0, k, other_a, 0));
        ends.push((*b, sides.1, k, other_b, 1));
    }
    let mut port: BTreeMap<(usize, usize), (f64, f64)> = BTreeMap::new(); // (edge, 0 from / 1 to) -> point
    let mut groups_by: BTreeMap<(usize, u8), Vec<(usize, f64, usize)>> = BTreeMap::new();
    for (node, side, k, other, which) in &ends {
        let s = match side {
            Side::MainEnd => 0u8,
            Side::MainStart => 1,
            Side::CrossEnd => 2,
        };
        groups_by
            .entry((*node, s))
            .or_default()
            .push((*k, *other, *which));
    }
    for ((node, s), mut list) in groups_by {
        list.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap().then(a.0.cmp(&b.0)));
        let (x0, y0, x1, y1) = rect_of(node);
        let k = list.len() as f64;
        for (j, (e, _, which)) in list.iter().enumerate() {
            let f = if k == 1.0 {
                0.5
            } else {
                0.25 + 0.5 * j as f64 / (k - 1.0)
            };
            let p = match (s, lr) {
                (0, true) => (x1, y0 + (y1 - y0) * f),
                (1, true) => (x0, y0 + (y1 - y0) * f),
                (2, true) => (x0 + (x1 - x0) * f, y1),
                (0, false) => (x0 + (x1 - x0) * f, y1),
                (1, false) => (x0 + (x1 - x0) * f, y0),
                _ => (x1, y0 + (y1 - y0) * f),
            };
            port.insert((*e, *which), p);
        }
    }

    // Markers.
    let mut marker_colors: Vec<&str> = Vec::new();
    for (_, _, e) in &valid_edges {
        let st = edge_style(e.kind, process);
        if st.arrow && !marker_colors.contains(&st.color) {
            marker_colors.push(st.color);
        }
    }
    for c in &marker_colors {
        d.def(&format!(
            r#"<marker id="{}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" markerUnits="userSpaceOnUse" orient="auto"><path d="M0 0L10 5L0 10z" fill="{c}"/></marker>"#,
            marker_id(c)
        ));
    }

    // Links.
    let mut node_boxes: Vec<(f64, f64, f64, f64)> = (0..nodes.len()).map(rect_of).collect();
    node_boxes.extend(frame_labels.iter().copied());
    let mut label_boxes: Vec<(f64, f64, f64, f64)> = Vec::new();
    let mut label_svg = String::new();
    for (k, (a, b, e)) in valid_edges.iter().enumerate() {
        let st = edge_style(e.kind, process);
        let p0 = port
            .get(&(k, 0))
            .copied()
            .unwrap_or((main_c(*a), cross_c(*a)));
        let p1 = port
            .get(&(k, 1))
            .copied()
            .unwrap_or((main_c(*b), cross_c(*b)));
        let sides = edge_sides[k];
        let (c0, c1) = if sides.0 == Side::CrossEnd {
            let bulge = 26.0 + ((if lr { p1.0 - p0.0 } else { p1.1 - p0.1 }).abs()) * 0.12;
            if lr {
                ((p0.0, p0.1 + bulge), (p1.0, p1.1 + bulge))
            } else {
                ((p0.0 + bulge, p0.1), (p1.0 + bulge, p1.1))
            }
        } else if lr {
            let dir = if p1.0 >= p0.0 { 1.0 } else { -1.0 };
            let kx = ((p1.0 - p0.0).abs() / 2.0).max(18.0) * dir;
            ((p0.0 + kx, p0.1), (p1.0 - kx, p1.1))
        } else {
            let dir = if p1.1 >= p0.1 { 1.0 } else { -1.0 };
            let ky = ((p1.1 - p0.1).abs() / 2.0).max(18.0) * dir;
            ((p0.0, p0.1 + ky), (p1.0, p1.1 - ky))
        };
        // Straight when aligned.
        let straight = if lr {
            (p0.1 - p1.1).abs() < 0.5
        } else {
            (p0.0 - p1.0).abs() < 0.5
        };
        let path = if straight && sides.0 != Side::CrossEnd {
            format!("M{} {}L{} {}", n(p0.0), n(p0.1), n(p1.0), n(p1.1))
        } else {
            format!(
                "M{} {}C{} {} {} {} {} {}",
                n(p0.0),
                n(p0.1),
                n(c0.0),
                n(c0.1),
                n(c1.0),
                n(c1.1),
                n(p1.0),
                n(p1.1)
            )
        };
        let _ = write!(
            d_raw_buf(&mut d),
            r#"<path d="{path}" fill="none" stroke="{}" stroke-width="{}" stroke-linecap="round"{}{}/>"#,
            st.color,
            n(st.width),
            if st.dash.is_empty() {
                String::new()
            } else {
                format!(r#" stroke-dasharray="{}""#, st.dash)
            },
            if st.arrow {
                format!(r#" marker-end="url(#{})""#, marker_id(st.color))
            } else {
                String::new()
            }
        );
        // Label.
        if let Some(lbl) = e.label.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
            let fs = m.tiny;
            let t = truncate(
                lbl,
                if lr {
                    main_gap + main_ext * 0.8
                } else {
                    cross_ext * 1.2
                },
                fs,
                false,
            );
            let tw = width(&t, fs, false) + 8.0;
            let th = fs + 6.0;
            let bez = |t: f64| {
                let u = 1.0 - t;
                let x = u * u * u * p0.0
                    + 3.0 * u * u * t * c0.0
                    + 3.0 * u * t * t * c1.0
                    + t * t * t * p1.0;
                let yv = u * u * u * p0.1
                    + 3.0 * u * u * t * c0.1
                    + 3.0 * u * t * t * c1.1
                    + t * t * t * p1.1;
                (x, yv)
            };
            let hits = |bx: (f64, f64, f64, f64), list: &[(f64, f64, f64, f64)]| {
                list.iter()
                    .any(|o| bx.0 < o.2 && o.0 < bx.2 && bx.1 < o.3 && o.1 < bx.3)
            };
            // A label must not sit on a frame border: fully inside a frame (clear of its edge) or fully
            // outside it.
            let on_border = |bx: (f64, f64, f64, f64)| {
                frame_rects
                    .iter()
                    .filter(|f| {
                        let inside = bx.0 >= f.0 + 3.0
                            && bx.2 <= f.2 - 3.0
                            && bx.1 >= f.1 + 3.0
                            && bx.3 <= f.3 - 3.0;
                        let outside = bx.2 <= f.0 - 3.0
                            || bx.0 >= f.2 + 3.0
                            || bx.3 <= f.1 - 3.0
                            || bx.1 >= f.3 + 3.0;
                        !inside && !outside
                    })
                    .count()
            };
            let count = |bx: (f64, f64, f64, f64), list: &[(f64, f64, f64, f64)]| {
                list.iter()
                    .filter(|o| bx.0 < o.2 && o.0 < bx.2 && bx.1 < o.3 && o.1 < bx.3)
                    .count()
            };
            let _ = &hits;
            // Positions along the link, from the middle outwards; the first that is clear wins, else the
            // one with the fewest conflicts.
            let mut best: Option<((f64, f64, f64, f64), usize)> = None;
            for k in 0..17 {
                let off = ((k + 1) / 2) as f64 * 0.05 * if k % 2 == 0 { 1.0 } else { -1.0 };
                let (cx, cy) = bez(0.5 + off);
                let bx = (cx - tw / 2.0, cy - th / 2.0, cx + tw / 2.0, cy + th / 2.0);
                let cost =
                    count(bx, &node_boxes) * 4 + count(bx, &label_boxes) * 3 + on_border(bx) * 2;
                if best.is_none_or(|(_, c)| cost < c) {
                    best = Some((bx, cost));
                }
                if cost == 0 {
                    break;
                }
            }
            let bx = best.unwrap().0;
            label_boxes.push(bx);
            let _ = write!(
                label_svg,
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="3" fill="white" stroke="{}" stroke-width="0.8"/><text x="{}" y="{}" font-size="{}" fill="{}" text-anchor="middle">{}</text>"#,
                n(bx.0),
                n(bx.1),
                n(bx.2 - bx.0),
                n(bx.3 - bx.1),
                theme::GRID,
                n((bx.0 + bx.2) / 2.0),
                n((bx.1 + bx.3) / 2.0 + fs * 0.35),
                n(fs),
                theme::INK,
                esc(&t)
            );
        }
    }

    // Nodes.
    let mut step_no: Vec<(usize, usize)> = Vec::new();
    if process {
        let mut ord: Vec<usize> = (0..nodes.len()).collect();
        ord.sort_by(|a, b| {
            layer[*a]
                .cmp(&layer[*b])
                .then(cross_c(*a).partial_cmp(&cross_c(*b)).unwrap())
                .then(a.cmp(b))
        });
        for (k, i) in ord.iter().enumerate() {
            step_no.push((*i, k + 1));
        }
    }
    for (i, nd) in nodes.iter().enumerate() {
        let (x0, y0, _, _) = rect_of(i);
        let (stroke, tint) = icons::colors(&nd.icon);
        let product = nd.sku.is_some();
        let (stroke, fill, sw) = if product {
            (theme::RED, theme::RED_TINT, 2.0)
        } else {
            (stroke, "white", 1.4)
        };
        let _ = tint;
        if nd.count > 1 {
            d.rect(
                x0 + 5.0,
                y0 - 5.0,
                geo.w,
                geo.h,
                8.0,
                &format!(r#"fill="white" stroke="{stroke}" stroke-width="1" stroke-opacity="0.6""#),
            );
            d.rect(
                x0 + 2.5,
                y0 - 2.5,
                geo.w,
                geo.h,
                8.0,
                &format!(r#"fill="white" stroke="{stroke}" stroke-width="1" stroke-opacity="0.8""#),
            );
        }
        d.rect(
            x0,
            y0,
            geo.w,
            geo.h,
            8.0,
            &format!(
                r#"fill="{fill}" stroke="{stroke}" stroke-width="{}""#,
                n(sw)
            ),
        );
        let lines = &node_lines[i];
        let bold = nd.sku.is_some();
        if geo.icon_left {
            let ix = x0 + pad_in;
            let iy = y0 + (geo.h - geo.icon) / 2.0;
            d.raw(&icons::draw(&nd.icon, ix, iy, geo.icon, None));
            let tx = ix + geo.icon + pad_in;
            let ty = y0 + geo.h / 2.0 - (lines.len() as f64 - 1.0) * lh / 2.0 + size * 0.35;
            let st = TextStyle::new(size, theme::INK);
            d.lines(tx, ty, lines, lh, if bold { st.bold() } else { st });
            let _ = geo.text_w;
        } else {
            let ix = x0 + (geo.w - geo.icon) / 2.0;
            d.raw(&icons::draw(&nd.icon, ix, y0 + pad_in, geo.icon, None));
            let ty = y0 + pad_in + geo.icon + 6.0 + size * 0.9;
            let st = TextStyle::new(size, theme::INK).anchor(Anchor::Middle);
            d.lines(
                x0 + geo.w / 2.0,
                ty,
                lines,
                lh,
                if bold { st.bold() } else { st },
            );
        }
        if nd.count > 1 {
            let badge = format!("x{}", nd.count);
            let bw = width(&badge, m.tiny, true) + 10.0;
            let bx = x0 + geo.w - bw + 8.0;
            let by = y0 - 9.0;
            d.rect(
                bx,
                by,
                bw,
                m.tiny + 6.0,
                (m.tiny + 6.0) / 2.0,
                &format!(r#"fill="{stroke}""#),
            );
            d.text(
                bx + bw / 2.0,
                by + m.tiny + 1.5,
                &badge,
                TextStyle::new(m.tiny, "white")
                    .bold()
                    .anchor(Anchor::Middle),
            );
        }
        if let Some((_, no)) = step_no.iter().find(|(j, _)| *j == i) {
            let r = m.tiny * 0.95;
            d.circle(x0, y0, r, &format!(r#"fill="{}""#, theme::INK));
            d.text(
                x0,
                y0 + m.tiny * 0.35,
                &no.to_string(),
                TextStyle::new(m.tiny, "white")
                    .bold()
                    .anchor(Anchor::Middle),
            );
        }
    }
    d.raw(&label_svg);

    // Bottom of the drawing.
    let mut bottom: f64 = top;
    for i in 0..nodes.len() {
        bottom = bottom.max(rect_of(i).3);
    }
    for (b, e) in band_ext.iter().enumerate() {
        if *e > 0.0 && lr {
            bottom = bottom.max(cross0 + band_start[b] + e);
        }
    }
    if !lr && (any_frame || lanes) {
        bottom = bottom.max(main_end + fp);
    }
    for (k, (a, b, _)) in valid_edges.iter().enumerate() {
        if edge_sides[k].0 == Side::CrossEnd && lr {
            let p0 = port.get(&(k, 0)).copied().unwrap_or((0.0, 0.0));
            let p1 = port.get(&(k, 1)).copied().unwrap_or((0.0, 0.0));
            let bulge = 26.0 + (p1.0 - p0.0).abs() * 0.12;
            bottom = bottom.max(p0.1.max(p1.1) + bulge * 0.75 + 4.0);
        }
        let _ = (a, b);
    }
    for bx in &label_boxes {
        bottom = bottom.max(bx.3);
    }
    let mut yb = bottom + 12.0;

    // Legend of the link kinds used.
    let mut kinds: Vec<EdgeKind> = Vec::new();
    for (_, _, e) in &valid_edges {
        if !kinds.contains(&e.kind) {
            kinds.push(e.kind);
        }
    }
    if !kinds.is_empty() && !(process && kinds.len() == 1) {
        let fs = m.small;
        let mut x = m.pad;
        let row_h = fs * 1.7;
        let mut row = 0.0;
        for k in &kinds {
            let st = edge_style(*k, process);
            let label = i18n::word(k.as_str(), lang);
            let w = 26.0 + 6.0 + width(label, fs, false);
            if x > m.pad && x + w > m.width - m.pad {
                x = m.pad;
                row += 1.0;
            }
            let cy = yb + row * row_h + row_h / 2.0;
            let _ = write!(
                d_raw_buf(&mut d),
                r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}" stroke-linecap="round"{}{}/>"#,
                n(x),
                n(cy),
                n(x + 26.0),
                n(cy),
                st.color,
                n(st.width),
                if st.dash.is_empty() {
                    String::new()
                } else {
                    format!(r#" stroke-dasharray="{}""#, st.dash)
                },
                if st.arrow {
                    format!(r#" marker-end="url(#{})""#, marker_id(st.color))
                } else {
                    String::new()
                }
            );
            d.text(
                x + 32.0,
                cy + fs * 0.35,
                label,
                TextStyle::new(fs, theme::MUTED),
            );
            x += w + 18.0;
        }
        yb += (row + 1.0) * row_h;
    }
    // A diagram wider than the page (very many nodes in TB) widens the canvas instead of overlapping.
    let mut max_x: f64 = m.width;
    for i in 0..nodes.len() {
        max_x = max_x.max(rect_of(i).2 + m.pad + 6.0);
    }
    d.width = max_x;
    d.height = yb + m.pad - 6.0;
    let _ = text::ELLIPSIS;
    Ok(d.finish(&spec.title))
}

/// Raw access to the document body for `write!`.
fn d_raw_buf(d: &mut Doc) -> &mut String {
    d.body_mut()
}
