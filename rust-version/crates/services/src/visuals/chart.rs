//! Charts: bar, stacked_bar, hbar, line, area, combo, waterfall, donut, radar, heatmap, scatter, gantt and
//! milestones. Layout runs top to bottom (title, top legend, plot, axis labels, bottom legend) with every
//! label measured against the bundled font, so labels wrap, thin out or get an ellipsis instead of
//! overlapping. Same spec, same bytes.

use std::f64::consts::PI;

use super::i18n::{self, number};
use super::spec::*;
use super::svg::{esc, n, Anchor, Doc, TextStyle};
use super::text::{self, truncate, width, wrap};
use super::theme::{self, Metrics};
use super::{RenderOptions, Result, VisualError};

// ---------------------------------------------------------------------------------------------------------
// Shared pieces
// ---------------------------------------------------------------------------------------------------------

struct Ctx<'a> {
    spec: &'a ChartSpec,
    m: Metrics,
    d: Doc,
    y: f64,
    lang: Language,
}

impl<'a> Ctx<'a> {
    fn new(spec: &'a ChartSpec, opts: RenderOptions) -> Ctx<'a> {
        let m = theme::metrics(opts.size);
        Ctx {
            spec,
            m,
            d: Doc::new(m.width),
            y: m.pad,
            lang: spec.language,
        }
    }

    fn inner_w(&self) -> f64 {
        self.m.width - 2.0 * self.m.pad
    }

    fn plot_h(&self) -> f64 {
        if self.m.width < 500.0 {
            190.0
        } else {
            230.0
        }
    }

    fn title(&mut self) {
        let t = self.spec.title.trim();
        if t.is_empty() {
            return;
        }
        let lines = wrap(t, self.inner_w(), self.m.title, true, 2);
        let lh = self.m.title * 1.3;
        self.d.lines(
            self.m.pad,
            self.y + self.m.title,
            &lines,
            lh,
            TextStyle::new(self.m.title, theme::INK).bold(),
        );
        self.y += lines.len() as f64 * lh + 8.0;
    }

    fn finish(mut self) -> String {
        self.d.height = self.y + self.m.pad;
        let title = self.spec.title.clone();
        self.d.finish(&title)
    }

    fn style(&self) -> TextStyle<'static> {
        TextStyle::new(self.m.small, theme::MUTED)
    }

    fn fmt(&self, v: f64, decimals: usize) -> String {
        let s = number(v, decimals, self.lang);
        if self.spec.percent {
            format!("{s}%")
        } else {
            s
        }
    }
}

/// A legend item: swatch kind and colour.
#[derive(Clone)]
struct Item {
    label: String,
    color: String,
    line: bool,
}

/// Legend laid out in rows that wrap; returns the height used. Drawn at `y` (top).
fn legend_rows(c: &mut Ctx, items: &[Item], x0: f64, y: f64, max_w: f64) -> f64 {
    if items.is_empty() {
        return 0.0;
    }
    let size = c.m.small;
    let sw = 12.0;
    let gap = 16.0;
    let row_h = size * 1.6;
    let max_label = (max_w - sw - 6.0).min(220.0);
    let mut x = x0;
    let mut row = 0.0;
    for it in items {
        let label = truncate(&it.label, max_label, size, false);
        let w = sw + 6.0 + width(&label, size, false);
        if x > x0 && x + w > x0 + max_w {
            x = x0;
            row += 1.0;
        }
        let cy = y + row * row_h + row_h / 2.0;
        swatch(&mut c.d, x, cy, sw, it);
        c.d.text(
            x + sw + 6.0,
            cy + size * 0.35,
            &label,
            TextStyle::new(size, theme::INK),
        );
        x += w + gap;
    }
    (row + 1.0) * row_h
}

fn swatch(d: &mut Doc, x: f64, cy: f64, sw: f64, it: &Item) {
    if it.line {
        d.line(
            x,
            cy,
            x + sw,
            cy,
            &format!(
                r#"stroke="{}" stroke-width="2.5" stroke-linecap="round""#,
                it.color
            ),
        );
        d.circle(x + sw / 2.0, cy, 2.6, &format!(r#"fill="{}""#, it.color));
    } else {
        d.rect(
            x,
            cy - sw / 2.0 + 1.0,
            sw,
            sw - 2.0,
            2.0,
            &format!(r#"fill="{}""#, it.color),
        );
    }
}

/// A full width legend at `y`; returns its height.
fn legend_at(c: &mut Ctx, items: &[Item], y: f64) -> f64 {
    let (x, w) = (c.m.pad, c.inner_w());
    legend_rows(c, items, x, y, w)
}

/// Vertical legend (right side); returns its width.
fn legend_column(c: &mut Ctx, items: &[Item], x0: f64, y: f64, max_w: f64) -> f64 {
    let size = c.m.small;
    let sw = 12.0;
    let row_h = size * 1.7;
    let mut used: f64 = 0.0;
    for (i, it) in items.iter().enumerate() {
        let label = truncate(&it.label, max_w - sw - 6.0, size, false);
        let cy = y + i as f64 * row_h + row_h / 2.0;
        swatch(&mut c.d, x0, cy, sw, it);
        c.d.text(
            x0 + sw + 6.0,
            cy + size * 0.35,
            &label,
            TextStyle::new(size, theme::INK),
        );
        used = used.max(sw + 6.0 + width(&label, size, false));
    }
    used
}

fn legend_column_width(m: &Metrics, items: &[Item], max_w: f64) -> f64 {
    items
        .iter()
        .map(|it| {
            18.0 + width(
                &truncate(&it.label, max_w - 18.0, m.small, false),
                m.small,
                false,
            )
        })
        .fold(0.0, f64::max)
}

/// Where the legend goes, if any.
fn legend_pos(spec: &ChartSpec, default_shown: bool) -> Option<LegendPosition> {
    match spec.legend {
        Some(LegendPosition::None) => None,
        Some(p) => Some(p),
        None if default_shown => Some(LegendPosition::Bottom),
        None => None,
    }
}

/// A "nice" axis: (low, high, step) covering [min, max] in about `target` steps.
pub fn nice_scale(min: f64, max: f64, target: usize) -> (f64, f64, f64) {
    let (mut lo, mut hi) = (min.min(max), min.max(max));
    if !lo.is_finite() || !hi.is_finite() {
        return (0.0, 1.0, 0.2);
    }
    if (hi - lo).abs() < 1e-12 {
        if hi == 0.0 {
            hi = 1.0;
        } else if hi > 0.0 {
            lo = lo.min(0.0);
            if lo == hi {
                hi = lo + 1.0;
            }
        } else {
            hi = 0.0;
        }
    }
    let raw = (hi - lo) / target.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let step = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 2.5 {
        2.5
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    } * mag;
    let lo2 = (lo / step + 1e-9).floor() * step;
    let hi2 = (hi / step - 1e-9).ceil() * step;
    (lo2, hi2, step)
}

/// Decimals needed to print ticks `step` apart (2.5 needs one, 0.25 two).
fn decimals_for(step: f64) -> usize {
    if step <= 0.0 || !step.is_finite() {
        return 0;
    }
    for d in 0..=4usize {
        let p = 10f64.powi(d as i32) * step;
        if (p - p.round()).abs() < 1e-6 * p.abs().max(1.0) {
            return d;
        }
    }
    4
}

/// Decimals for data values (up to 2, only as many as the data has).
fn value_decimals(vals: &[f64]) -> usize {
    let mut d = 0;
    for v in vals {
        for k in 0..=2 {
            let p = 10f64.powi(k as i32);
            if ((v * p).round() - v * p).abs() < 1e-6 {
                d = d.max(k);
                break;
            }
            if k == 2 {
                d = 2;
            }
        }
    }
    d
}

struct Scale {
    lo: f64,
    hi: f64,
    step: f64,
    /// pixel range: value lo maps to p0, hi to p1
    p0: f64,
    p1: f64,
}

impl Scale {
    fn at(&self, v: f64) -> f64 {
        if self.hi == self.lo {
            return self.p0;
        }
        self.p0 + (v - self.lo) / (self.hi - self.lo) * (self.p1 - self.p0)
    }
    fn ticks(&self) -> Vec<f64> {
        let mut out = Vec::new();
        let k = ((self.hi - self.lo) / self.step).round() as i64;
        for i in 0..=k.min(40) {
            out.push(self.lo + i as f64 * self.step);
        }
        out
    }
}

/// How category labels under an axis are drawn.
struct CatLabels {
    lines: Vec<Vec<String>>,
    rotated: bool,
    every: usize,
    height: f64,
}

fn layout_cat_labels(cats: &[String], band: f64, size: f64, rot_max: f64) -> CatLabels {
    let lh = size * 1.2;
    let avail = (band - 6.0).max(8.0);
    // Horizontal, up to 2 lines without loss.
    let fits = |max_lines: usize| {
        cats.iter().all(|c| {
            let w = wrap(c, avail, size, false, max_lines);
            !w.iter().any(|l| l.ends_with(text::ELLIPSIS))
        })
    };
    for max_lines in [1usize, 2] {
        if fits(max_lines) {
            let lines: Vec<Vec<String>> = cats
                .iter()
                .map(|c| wrap(c, avail, size, false, max_lines))
                .collect();
            let nl = lines.iter().map(|l| l.len()).max().unwrap_or(1);
            return CatLabels {
                lines,
                rotated: false,
                every: 1,
                height: nl as f64 * lh + 6.0,
            };
        }
    }
    // Wide enough for 3 wrapped lines with an ellipsis: still horizontal.
    if avail >= 56.0 {
        let lines: Vec<Vec<String>> = cats
            .iter()
            .map(|c| wrap(c, avail, size, false, 3))
            .collect();
        let nl = lines.iter().map(|l| l.len()).max().unwrap_or(1);
        return CatLabels {
            lines,
            rotated: false,
            every: 1,
            height: nl as f64 * lh + 6.0,
        };
    }
    // Rotated -40 degrees, truncated; thin out when the bands are narrower than a line.
    let every = ((size * 1.25) / band).ceil().max(1.0) as usize;
    let lines: Vec<Vec<String>> = cats
        .iter()
        .map(|c| vec![truncate(c, rot_max, size, false)])
        .collect();
    let longest = lines
        .iter()
        .map(|l| width(&l[0], size, false))
        .fold(0.0, f64::max);
    let a = 40f64.to_radians();
    CatLabels {
        lines,
        rotated: true,
        every,
        height: longest * a.sin() + size * a.cos() + 8.0,
    }
}

fn draw_cat_labels(c: &mut Ctx, cl: &CatLabels, centers: &[f64], y: f64) {
    let size = c.m.small;
    let lh = size * 1.2;
    for (i, (lines, cx)) in cl.lines.iter().zip(centers).enumerate() {
        if i % cl.every != 0 {
            continue;
        }
        if cl.rotated {
            let ty = y + size;
            c.d.raw(&format!(
                r#"<text x="{}" y="{}" font-size="{}" fill="{}" text-anchor="end" transform="rotate(-40 {} {})">{}</text>"#,
                n(*cx + 3.0),
                n(ty),
                n(size),
                theme::MUTED,
                n(*cx + 3.0),
                n(ty),
                esc(&lines[0])
            ));
        } else {
            c.d.lines(
                *cx,
                y + size,
                lines,
                lh,
                TextStyle::new(size, theme::MUTED).anchor(Anchor::Middle),
            );
        }
    }
}

/// Y axis title or unit, written above the axis, left aligned (never rotated: easier to read).
fn axis_caption(spec: &ChartSpec, unit_word: Option<&str>) -> String {
    let unit = spec
        .unit
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(unit_word);
    let label = spec
        .y_label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match (label, unit) {
        (Some(l), Some(u)) if !l.to_lowercase().contains(&u.to_lowercase()) => format!("{l} ({u})"),
        (Some(l), _) => l.to_string(),
        (None, Some(u)) => u.to_string(),
        (None, None) => String::new(),
    }
}

fn x_caption(c: &mut Ctx, x_mid: f64) {
    if let Some(xl) = c
        .spec
        .x_label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let t = truncate(xl, c.inner_w(), c.m.small, false);
        c.d.text(
            x_mid,
            c.y + c.m.small,
            &t,
            TextStyle::new(c.m.small, theme::MUTED).anchor(Anchor::Middle),
        );
        c.y += c.m.small * 1.5;
    }
}

fn series_items(spec: &ChartSpec, line_for: impl Fn(&Series) -> bool) -> Vec<Item> {
    spec.series
        .iter()
        .enumerate()
        .map(|(i, s)| Item {
            label: s.name.clone(),
            color: theme::series_color(&spec.colors, i),
            line: line_for(s),
        })
        .collect()
}

/// Bounding boxes for simple collision checks.
#[derive(Clone, Copy)]
struct BoxR {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl BoxR {
    fn hits(&self, o: &BoxR) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }
}

fn text_box(x: f64, baseline: f64, s: &str, size: f64, anchor: Anchor) -> BoxR {
    let w = width(s, size, false);
    let x0 = match anchor {
        Anchor::Start => x,
        Anchor::Middle => x - w / 2.0,
        Anchor::End => x - w,
    };
    BoxR {
        x0: x0 - 2.0,
        y0: baseline - size,
        x1: x0 + w + 2.0,
        y1: baseline + size * 0.3,
    }
}

/// A label on a light pill so it stays readable over lines and bars.
fn pill_text(d: &mut Doc, x: f64, baseline: f64, s: &str, size: f64, fill: &str, anchor: Anchor) {
    let b = text_box(x, baseline, s, size, anchor);
    d.rect(
        b.x0 - 1.0,
        b.y0 - 1.0,
        b.x1 - b.x0 + 2.0,
        b.y1 - b.y0 + 2.0,
        3.0,
        r#"fill="white" fill-opacity="0.9""#,
    );
    d.text(x, baseline, s, TextStyle::new(size, fill).anchor(anchor));
}

// ---------------------------------------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------------------------------------

pub fn render(spec: &ChartSpec, opts: RenderOptions) -> Result<String> {
    let mut c = Ctx::new(spec, opts);
    c.title();
    match spec.kind {
        ChartKind::Bar
        | ChartKind::StackedBar
        | ChartKind::Line
        | ChartKind::Area
        | ChartKind::Combo
        | ChartKind::Waterfall => {
            need_categories(spec)?;
            category_chart(&mut c)
        }
        ChartKind::Hbar => {
            need_categories(spec)?;
            hbar(&mut c)
        }
        ChartKind::Donut => {
            need_categories(spec)?;
            donut(&mut c)
        }
        ChartKind::Radar => {
            need_categories(spec)?;
            radar(&mut c)
        }
        ChartKind::Heatmap => {
            need_categories(spec)?;
            heatmap(&mut c)
        }
        ChartKind::Scatter => scatter(&mut c),
        ChartKind::Gantt => gantt(&mut c),
        ChartKind::Milestones => milestones(&mut c),
    }?;
    Ok(c.finish())
}

/// The drawn points of the line series (for keeping labels off their markers).
fn line_series_points(
    spec: &ChartSpec,
    tops: &[Vec<(f64, f64)>],
    is_line: impl Fn(&Series) -> bool,
) -> Vec<(f64, f64)> {
    spec.series
        .iter()
        .enumerate()
        .filter(|(_, s)| is_line(s))
        .flat_map(|(i, _)| tops.get(i).cloned().unwrap_or_default())
        .collect()
}

fn need_categories(spec: &ChartSpec) -> Result<()> {
    if spec.categories.is_empty() || spec.series.is_empty() {
        return Err(VisualError::Invalid(
            "the chart needs categories and at least one series".into(),
        ));
    }
    Ok(())
}

fn val(s: &Series, i: usize) -> f64 {
    s.values
        .get(i)
        .copied()
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------------------------------------
// Vertical category charts: bar, stacked_bar, line, area, combo, waterfall
// ---------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Draw {
    Bar,
    Line,
}

fn category_chart(c: &mut Ctx) -> Result<()> {
    let spec = c.spec;
    let ncat = spec.categories.len();
    let kind = spec.kind;
    let stacked = kind == ChartKind::StackedBar || spec.stacked || spec.percent;
    let waterfall = kind == ChartKind::Waterfall;
    let is_area = kind == ChartKind::Area;
    let draw_of = |s: &Series| -> Draw {
        match kind {
            ChartKind::Line | ChartKind::Area => Draw::Line,
            ChartKind::Combo => {
                if s.series_type == Some(SeriesType::Line) {
                    Draw::Line
                } else {
                    Draw::Bar
                }
            }
            _ => Draw::Bar,
        }
    };
    let right_of = |s: &Series| kind == ChartKind::Combo && s.axis == Some(Axis::Right);
    let has_right = spec.series.iter().any(&right_of);

    // Values per series (percent: share of the category total over the stacked series).
    let mut vals: Vec<Vec<f64>> = spec
        .series
        .iter()
        .map(|s| (0..ncat).map(|i| val(s, i)).collect())
        .collect();
    if spec.percent {
        for i in 0..ncat {
            let tot: f64 = vals.iter().map(|v| v[i]).sum();
            for v in vals.iter_mut() {
                v[i] = if tot > 0.0 { v[i] / tot * 100.0 } else { 0.0 };
            }
        }
    }

    // Waterfall: running totals.
    let wf_total = spec.show_total.unwrap_or(true);
    let wf_start = spec.start_total.unwrap_or(true);
    let mut wf: Vec<(f64, f64, i8)> = Vec::new(); // (from, to, sign: 1 up, -1 down, 0 total)
    let mut cats: Vec<String> = spec.categories.clone();
    if waterfall {
        let mut run = 0.0;
        for (i, &dv) in vals[0].iter().take(ncat).enumerate() {
            if i == 0 && wf_start {
                // The starting total, drawn from 0.
                wf.push((0.0, dv, 0));
            } else {
                wf.push((run, run + dv, if dv >= 0.0 { 1 } else { -1 }));
            }
            run += dv;
        }
        if wf_total {
            wf.push((0.0, run, 0));
            cats.push(i18n::word("total", c.lang).to_string());
        }
    }
    let nbands = cats.len();

    // Ranges (left axis and right axis).
    let (mut lmin, mut lmax, mut rmin, mut rmax) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    if waterfall {
        for (a, b, _) in &wf {
            lmin = lmin.min(a.min(*b));
            lmax = lmax.max(a.max(*b));
        }
    } else if stacked {
        #[allow(clippy::needless_range_loop)] // `i` indexes every series' values, not one list
        for i in 0..ncat {
            let (mut pos, mut neg) = (0.0, 0.0);
            for (si, s) in spec.series.iter().enumerate() {
                if right_of(s) {
                    continue;
                }
                let v = vals[si][i];
                if v >= 0.0 {
                    pos += v
                } else {
                    neg += v
                }
            }
            lmax = lmax.max(pos);
            lmin = lmin.min(neg);
        }
    } else {
        for (si, s) in spec.series.iter().enumerate() {
            for v in &vals[si] {
                if right_of(s) {
                    rmin = rmin.min(*v);
                    rmax = rmax.max(*v);
                } else {
                    lmin = lmin.min(*v);
                    lmax = lmax.max(*v);
                }
            }
        }
    }
    for a in &spec.annotations {
        if let Annotation::RefLine { axis, value, .. } = a {
            if axis == "y" {
                lmin = lmin.min(*value);
                lmax = lmax.max(*value);
            }
        }
    }
    if spec.percent {
        lmin = 0.0;
        lmax = 100.0;
    }
    let (llo, lhi, lstep) = nice_scale(lmin, lmax, 5);
    let (rlo, rhi, rstep) = nice_scale(rmin, rmax, 5);
    let ldec = decimals_for(lstep);
    let rdec = decimals_for(rstep);

    // Legend.
    let items: Vec<Item> = if waterfall {
        vec![
            Item {
                label: i18n::word("increase", c.lang).into(),
                color: theme::BLUE.into(),
                line: false,
            },
            Item {
                label: i18n::word("decrease", c.lang).into(),
                color: theme::AMBER.into(),
                line: false,
            },
        ]
        .into_iter()
        .chain((wf_total || wf_start).then(|| Item {
            label: i18n::word("total", c.lang).into(),
            color: theme::SLATE.into(),
            line: false,
        }))
        .collect()
    } else {
        series_items(spec, |s| draw_of(s) == Draw::Line)
    };
    let lpos = legend_pos(spec, items.len() > 1 || has_right);
    let mut right_legend_w = 0.0;
    if lpos == Some(LegendPosition::Right) {
        right_legend_w = legend_column_width(&c.m, &items, c.inner_w() * 0.3) + 16.0;
    }
    if lpos == Some(LegendPosition::Top) {
        let y0 = c.y;
        let h = legend_at(c, &items, y0);
        c.y += h + 6.0;
    }

    // Axis captions above the plot.
    let left_cap = axis_caption(spec, None);
    let right_cap = if has_right {
        spec.series
            .iter()
            .filter(|s| right_of(s))
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        String::new()
    };
    let tick_w = |lo: f64, hi: f64, step: f64, dec: usize| {
        let sc = Scale {
            lo,
            hi,
            step,
            p0: 0.0,
            p1: 1.0,
        };
        sc.ticks()
            .iter()
            .map(|t| width(&c.fmt(*t, dec), c.m.small, false))
            .fold(0.0, f64::max)
    };
    let left_m = c.m.pad + tick_w(llo, lhi, lstep, ldec) + 8.0;
    let right_m = c.m.pad
        + right_legend_w
        + if has_right {
            tick_w(rlo, rhi, rstep, rdec) + 8.0
        } else {
            4.0
        };
    let x0 = left_m;
    let x1 = c.m.width - right_m;
    let pw = x1 - x0;
    if !left_cap.is_empty() || !right_cap.is_empty() {
        let half = pw / 2.0 + 20.0;
        if !left_cap.is_empty() {
            let t = truncate(
                &left_cap,
                if has_right { half } else { c.inner_w() },
                c.m.small,
                false,
            );
            c.d.text(c.m.pad, c.y + c.m.small, &t, c.style());
        }
        if !right_cap.is_empty() {
            let t = truncate(&right_cap, half - 20.0, c.m.small, false);
            c.d.text(
                c.m.width - c.m.pad - right_legend_w,
                c.y + c.m.small,
                &t,
                c.style().anchor(Anchor::End),
            );
        }
        c.y += c.m.small * 1.6;
    }
    let top = c.y + 6.0;
    let ph = c.plot_h();
    let bottom = top + ph;
    let ls = Scale {
        lo: llo,
        hi: lhi,
        step: lstep,
        p0: bottom,
        p1: top,
    };
    let rs = Scale {
        lo: rlo,
        hi: rhi,
        step: rstep,
        p0: bottom,
        p1: top,
    };

    // Grid and ticks.
    for t in ls.ticks() {
        let y = ls.at(t);
        let zero = t.abs() < 1e-9;
        c.d.line(
            x0,
            y,
            x1,
            y,
            &format!(
                r#"stroke="{}" stroke-width="1""#,
                if zero { theme::AXIS } else { theme::GRID }
            ),
        );
        c.d.text(
            x0 - 6.0,
            y + c.m.small * 0.35,
            &c.fmt(t, ldec),
            TextStyle::new(c.m.small, theme::FAINT).anchor(Anchor::End),
        );
    }
    if has_right {
        for t in rs.ticks() {
            let y = rs.at(t);
            c.d.text(
                x1 + 6.0,
                y + c.m.small * 0.35,
                &number(t, rdec, c.lang),
                TextStyle::new(c.m.small, theme::FAINT),
            );
        }
    }

    let band = pw / nbands as f64;
    let centers: Vec<f64> = (0..nbands).map(|i| x0 + band * (i as f64 + 0.5)).collect();
    let bar_series: Vec<usize> = (0..spec.series.len())
        .filter(|&i| draw_of(&spec.series[i]) == Draw::Bar)
        .collect();
    let line_series: Vec<usize> = (0..spec.series.len())
        .filter(|&i| draw_of(&spec.series[i]) == Draw::Line)
        .collect();
    let group_w = band * if nbands == 1 { 0.45 } else { 0.72 };
    let n_bars = if stacked { 1 } else { bar_series.len().max(1) };
    let bar_w = (group_w / n_bars as f64).min(56.0);
    let vdec = value_decimals(&vals.iter().flatten().copied().collect::<Vec<_>>());
    let show_values =
        !stacked && (nbands * n_bars) <= 16 && bar_w >= width("0000", c.m.tiny, false) * 0.8;
    let mut placed: Vec<BoxR> = Vec::new();
    let mut bar_tops: Vec<Vec<(f64, f64)>> = vec![Vec::new(); spec.series.len()]; // per series, per category: (x, y top)

    let mut bar_labels: Vec<(f64, f64, String, bool)> = Vec::new(); // (x, baseline, text, above the bar)
    if waterfall {
        let bw = (band * 0.6).min(64.0);
        for (i, (a, b, sign)) in wf.iter().enumerate() {
            let col = match sign {
                1 => theme::BLUE,
                -1 => theme::AMBER,
                _ => theme::SLATE,
            };
            let ya = ls.at(*a);
            let yb = ls.at(*b);
            let x = centers[i] - bw / 2.0;
            c.d.rect(
                x,
                ya.min(yb),
                bw,
                (ya - yb).abs().max(1.0),
                2.0,
                &format!(r#"fill="{col}""#),
            );
            if i + 1 < wf.len() {
                let nx = centers[i + 1] - bw / 2.0;
                c.d.line(
                    x + bw,
                    yb,
                    nx,
                    yb,
                    &format!(
                        r#"stroke="{}" stroke-width="1" stroke-dasharray="3 3""#,
                        theme::AXIS
                    ),
                );
            }
            let dv = b - a;
            let label = if *sign == 0 {
                number(*b, vdec, c.lang)
            } else if dv >= 0.0 {
                format!("+{}", number(dv, vdec, c.lang))
            } else {
                number(dv, vdec, c.lang)
            };
            if width(&label, c.m.tiny, false) <= band - 2.0 {
                let ly = ya.min(yb) - 4.0;
                c.d.text(
                    centers[i],
                    ly,
                    &label,
                    TextStyle::new(c.m.tiny, theme::INK).anchor(Anchor::Middle),
                );
                placed.push(text_box(centers[i], ly, &label, c.m.tiny, Anchor::Middle));
            }
            bar_tops[0].push((centers[i], ya.min(yb)));
        }
    } else {
        // Bars.
        for i in 0..ncat {
            let (mut pos, mut neg) = (0.0f64, 0.0f64);
            for (k, &si) in bar_series.iter().enumerate() {
                let s = &spec.series[si];
                let sc = if right_of(s) { &rs } else { &ls };
                let col = theme::series_color(&spec.colors, si);
                let v = vals[si][i];
                let (ya, yb, x) = if stacked {
                    let (a, b) = if v >= 0.0 {
                        let r = (pos, pos + v);
                        pos += v;
                        r
                    } else {
                        let r = (neg, neg + v);
                        neg += v;
                        r
                    };
                    (sc.at(a), sc.at(b), centers[i] - bar_w / 2.0)
                } else {
                    let gx = centers[i] - (bar_w * bar_series.len() as f64) / 2.0;
                    (sc.at(0.0f64.max(sc.lo)), sc.at(v), gx + k as f64 * bar_w)
                };
                let inset = if stacked || bar_series.len() == 1 {
                    0.0
                } else {
                    1.0
                };
                let h = (ya - yb).abs();
                if h > 0.0 {
                    c.d.rect(
                        x + inset,
                        ya.min(yb),
                        bar_w - 2.0 * inset,
                        h,
                        if h > 4.0 { 2.0 } else { 0.0 },
                        &format!(r#"fill="{col}""#),
                    );
                }
                // Stacked: white separators keep segments apart.
                if stacked && h > 0.0 {
                    c.d.line(
                        x,
                        ya.min(yb),
                        x + bar_w,
                        ya.min(yb),
                        r#"stroke="white" stroke-width="1""#,
                    );
                }
                bar_tops[si].push((x + bar_w / 2.0, ya.min(yb)));
                if show_values {
                    let label = number(v, vdec, c.lang);
                    let ly = if v >= 0.0 {
                        yb - 4.0
                    } else {
                        yb + c.m.tiny + 3.0
                    };
                    let bx = text_box(x + bar_w / 2.0, ly, &label, c.m.tiny, Anchor::Middle);
                    if bx.x1 - bx.x0 <= bar_w + 6.0 && !placed.iter().any(|p| p.hits(&bx)) {
                        // Drawn after the lines, so a line or marker never hides it.
                        bar_labels.push((x + bar_w / 2.0, ly, label, v >= 0.0));
                        placed.push(bx);
                    }
                }
            }
            // Stacked totals above the stack.
            if stacked
                && !spec.percent
                && bar_series.len() > 1
                && band >= width("00000", c.m.tiny, false)
            {
                let label = number(pos + neg, vdec, c.lang);
                let ly = ls.at(pos) - 4.0;
                c.d.text(
                    centers[i],
                    ly,
                    &label,
                    TextStyle::new(c.m.tiny, theme::INK)
                        .bold()
                        .anchor(Anchor::Middle),
                );
            }
        }
        // Lines and areas.
        let mut base = vec![0.0f64; ncat];
        for &si in &line_series {
            let s = &spec.series[si];
            let sc = if right_of(s) { &rs } else { &ls };
            let col = theme::series_color(&spec.colors, si);
            let pts: Vec<(f64, f64)> = (0..ncat)
                .map(|i| {
                    let v = vals[si][i] + if is_area && stacked { base[i] } else { 0.0 };
                    (centers[i], sc.at(v))
                })
                .collect();
            if is_area {
                let mut d = String::new();
                for (k, (x, y)) in pts.iter().enumerate() {
                    d.push_str(&format!(
                        "{}{} {}",
                        if k == 0 { "M" } else { "L" },
                        n(*x),
                        n(*y)
                    ));
                }
                for i in (0..ncat).rev() {
                    let b = if stacked { base[i] } else { 0.0f64.max(sc.lo) };
                    d.push_str(&format!("L{} {}", n(centers[i]), n(sc.at(b))));
                }
                d.push('Z');
                c.d.path(
                    &d,
                    &format!(
                        r#"fill="{col}" fill-opacity="{}""#,
                        if stacked { "0.55" } else { "0.16" }
                    ),
                );
            }
            let mut d = String::new();
            for (k, (x, y)) in pts.iter().enumerate() {
                d.push_str(&format!(
                    "{}{} {}",
                    if k == 0 { "M" } else { "L" },
                    n(*x),
                    n(*y)
                ));
            }
            c.d.path(&d, &format!(r#"fill="none" stroke="{col}" stroke-width="2.2" stroke-linejoin="round" stroke-linecap="round""#));
            if ncat <= 24 {
                for (x, y) in &pts {
                    c.d.circle(
                        *x,
                        *y,
                        3.0,
                        &format!(r#"fill="white" stroke="{col}" stroke-width="2""#),
                    );
                }
            }
            if is_area && stacked {
                for i in 0..ncat {
                    base[i] += vals[si][i];
                }
            }
            bar_tops[si] = pts;
            // Values over a single line when there is room.
            if line_series.len() == 1 && bar_series.is_empty() && ncat <= 12 {
                for (i, (x, y)) in bar_tops[si].clone().iter().enumerate() {
                    let label = number(vals[si][i], vdec, c.lang);
                    let bx = text_box(*x, y - 8.0, &label, c.m.tiny, Anchor::Middle);
                    if !placed.iter().any(|p| p.hits(&bx)) && bx.y0 > top - 4.0 {
                        c.d.text(
                            *x,
                            y - 8.0,
                            &label,
                            TextStyle::new(c.m.tiny, theme::INK).anchor(Anchor::Middle),
                        );
                        placed.push(bx);
                    }
                }
            }
        }
    }
    // Bar values: moved clear of any line marker, on a white halo so lines crossing them stay readable.
    let markers: Vec<BoxR> = line_series_points(spec, &bar_tops, |s| draw_of(s) == Draw::Line)
        .iter()
        .map(|(x, y)| BoxR {
            x0: x - 5.0,
            y0: y - 5.0,
            x1: x + 5.0,
            y1: y + 5.0,
        })
        .collect();
    for (x, ly, label, above) in &bar_labels {
        let mut ly = *ly;
        for _ in 0..3 {
            let bx = text_box(*x, ly, label, c.m.tiny, Anchor::Middle);
            let Some(mk) = markers.iter().find(|m| m.hits(&bx)) else {
                break;
            };
            ly = if *above && mk.y0 - 3.0 - c.m.tiny >= top - 2.0 {
                mk.y0 - 3.0
            } else {
                mk.y1 + c.m.tiny + 2.0
            };
        }
        c.d.text(
            *x,
            ly,
            label,
            TextStyle::new(c.m.tiny, theme::INK)
                .anchor(Anchor::Middle)
                .halo(),
        );
    }

    // Axis base line.
    c.d.line(
        x0,
        bottom,
        x1,
        bottom,
        &format!(r#"stroke="{}" stroke-width="1""#, theme::AXIS),
    );

    // Annotations.
    annotations_xy(
        c,
        &ls,
        x0,
        x1,
        top,
        bottom,
        &centers,
        &bar_tops,
        &mut placed,
    );

    // Category labels.
    let cl = layout_cat_labels(
        &cats,
        band,
        c.m.small,
        if c.m.width < 500.0 { 90.0 } else { 120.0 },
    );
    draw_cat_labels(c, &cl, &centers, bottom + 4.0);
    c.y = bottom + 4.0 + cl.height;
    x_caption(c, (x0 + x1) / 2.0);

    if let Some(p) = lpos {
        match p {
            LegendPosition::Bottom => {
                c.y += 4.0;
                let y0 = c.y;
                let h = legend_at(c, &items, y0);
                c.y += h;
            }
            LegendPosition::Right => {
                let lx = c.m.width - c.m.pad - right_legend_w + 16.0;
                legend_column(c, &items, lx, top, right_legend_w - 16.0);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Reference lines, highlights and notes on a category chart (x = category index, y = value).
#[allow(clippy::too_many_arguments)]
fn annotations_xy(
    c: &mut Ctx,
    ys: &Scale,
    x0: f64,
    x1: f64,
    top: f64,
    bottom: f64,
    centers: &[f64],
    tops: &[Vec<(f64, f64)>],
    placed: &mut Vec<BoxR>,
) {
    let spec = c.spec;
    let xat = |v: f64| -> f64 {
        if centers.is_empty() {
            return x0;
        }
        let band = (x1 - x0) / centers.len() as f64;
        (x0 + band * (v + 0.5)).clamp(x0, x1)
    };
    for a in &spec.annotations {
        match a {
            Annotation::RefLine { axis, value, label } if axis == "y" => {
                let y = ys.at(*value);
                c.d.line(
                    x0,
                    y,
                    x1,
                    y,
                    &format!(
                        r#"stroke="{}" stroke-width="1.4" stroke-dasharray="6 4""#,
                        theme::MAROON
                    ),
                );
                let t = truncate(label, (x1 - x0) * 0.6, c.m.tiny, false);
                if !t.is_empty() {
                    let ly = if y - 4.0 - c.m.tiny < top {
                        y + c.m.tiny + 3.0
                    } else {
                        y - 4.0
                    };
                    pill_text(
                        &mut c.d,
                        x1 - 2.0,
                        ly,
                        &t,
                        c.m.tiny,
                        theme::MAROON,
                        Anchor::End,
                    );
                    placed.push(text_box(x1 - 2.0, ly, &t, c.m.tiny, Anchor::End));
                }
            }
            Annotation::RefLine { value, label, .. } => {
                let x = xat(*value);
                c.d.line(
                    x,
                    top,
                    x,
                    bottom,
                    &format!(
                        r#"stroke="{}" stroke-width="1.4" stroke-dasharray="6 4""#,
                        theme::MAROON
                    ),
                );
                let t = truncate(label, (x1 - x0) * 0.4, c.m.tiny, false);
                if !t.is_empty() {
                    let anchor = if x > (x0 + x1) / 2.0 {
                        Anchor::End
                    } else {
                        Anchor::Start
                    };
                    let tx = if anchor == Anchor::End {
                        x - 4.0
                    } else {
                        x + 4.0
                    };
                    pill_text(
                        &mut c.d,
                        tx,
                        top + c.m.tiny + 2.0,
                        &t,
                        c.m.tiny,
                        theme::MAROON,
                        anchor,
                    );
                }
            }
            Annotation::Highlight {
                series,
                index,
                label,
            } => {
                let Some(si) = spec.series.iter().position(|s| &s.name == series) else {
                    continue;
                };
                let Some(&(x, y)) = tops.get(si).and_then(|t| t.get(*index)) else {
                    continue;
                };
                c.d.circle(
                    x,
                    y,
                    7.0,
                    &format!(r#"fill="none" stroke="{}" stroke-width="2""#, theme::MAROON),
                );
                let t = truncate(label, 160.0, c.m.tiny, false);
                if !t.is_empty() {
                    let mut ly = (y - 14.0).max(top + c.m.tiny);
                    let mut bx = text_box(x, ly, &t, c.m.tiny, Anchor::Middle);
                    if placed.iter().any(|p| p.hits(&bx)) {
                        ly = y + 20.0;
                        bx = text_box(x, ly, &t, c.m.tiny, Anchor::Middle);
                    }
                    let shift = if bx.x0 < x0 {
                        x0 - bx.x0
                    } else if bx.x1 > x1 {
                        x1 - bx.x1
                    } else {
                        0.0
                    };
                    pill_text(
                        &mut c.d,
                        x + shift,
                        ly,
                        &t,
                        c.m.tiny,
                        theme::MAROON,
                        Anchor::Middle,
                    );
                    placed.push(bx);
                }
            }
            Annotation::Note { text, at } => {
                let x = xat(at[0]);
                let y = ys.at(at[1]).clamp(top + c.m.tiny, bottom - 2.0);
                let t = truncate(text, (x1 - x0) * 0.5, c.m.tiny, false);
                let anchor = if x > (x0 + x1) / 2.0 {
                    Anchor::End
                } else {
                    Anchor::Start
                };
                pill_text(&mut c.d, x, y, &t, c.m.tiny, theme::INK, anchor);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Horizontal bars
// ---------------------------------------------------------------------------------------------------------

fn hbar(c: &mut Ctx) -> Result<()> {
    let spec = c.spec;
    let ncat = spec.categories.len();
    let stacked = spec.stacked || spec.percent;
    let mut vals: Vec<Vec<f64>> = spec
        .series
        .iter()
        .map(|s| (0..ncat).map(|i| val(s, i)).collect())
        .collect();
    if spec.percent {
        for i in 0..ncat {
            let tot: f64 = vals.iter().map(|v| v[i]).sum();
            for v in vals.iter_mut() {
                v[i] = if tot > 0.0 { v[i] / tot * 100.0 } else { 0.0 };
            }
        }
    }
    let (mut lo, mut hi) = (0.0f64, 0.0f64);
    for i in 0..ncat {
        if stacked {
            hi = hi.max(vals.iter().map(|v| v[i].max(0.0)).sum());
            lo = lo.min(vals.iter().map(|v| v[i].min(0.0)).sum());
        } else {
            for v in &vals {
                hi = hi.max(v[i]);
                lo = lo.min(v[i]);
            }
        }
    }
    for a in &spec.annotations {
        if let Annotation::RefLine { axis, value, .. } = a {
            if axis == "y" {
                lo = lo.min(*value);
                hi = hi.max(*value);
            }
        }
    }
    if spec.percent {
        lo = 0.0;
        hi = 100.0;
    }
    let (slo, shi, step) = nice_scale(lo, hi, if c.m.width < 500.0 { 4 } else { 6 });
    let dec = decimals_for(step);
    let items = series_items(spec, |_| false);
    let lpos = legend_pos(spec, items.len() > 1);
    if lpos == Some(LegendPosition::Top) {
        let y0 = c.y;
        let h = legend_at(c, &items, y0);
        c.y += h + 6.0;
    }
    let cap = axis_caption(spec, None);
    // Label column.
    let size = c.m.small;
    let col_max = c.inner_w() * 0.36;
    let labels: Vec<Vec<String>> = spec
        .categories
        .iter()
        .map(|s| wrap(s, col_max, size, false, 2))
        .collect();
    let col_w = labels
        .iter()
        .map(|l| text::max_width(l, size, false))
        .fold(0.0, f64::max)
        .min(col_max);
    let two = labels.iter().any(|l| l.len() > 1);
    let nb = if stacked { 1 } else { spec.series.len() };
    let bar_h = if nb == 1 {
        16.0
    } else {
        (26.0 / nb as f64).max(7.0)
    };
    let row_h = (bar_h * nb as f64 + 10.0).max(if two {
        size * 2.5 + 6.0
    } else {
        size * 1.4 + 10.0
    });
    let x0 = c.m.pad + col_w + 10.0;
    let vdec = value_decimals(&vals.iter().flatten().copied().collect::<Vec<_>>());
    let val_room = if stacked {
        0.0
    } else {
        width(&c.fmt(shi, vdec), c.m.tiny, false) + 8.0
    };
    let x1 = c.m.width - c.m.pad - val_room.max(width(&c.fmt(shi, dec), size, false) / 2.0);
    let has_ref = spec.annotations.iter().any(|a| matches!(a, Annotation::RefLine { axis, label, .. } if axis == "y" && !label.trim().is_empty()));
    let top = c.y + 4.0 + if has_ref { c.m.tiny + 8.0 } else { 0.0 };
    let sc = Scale {
        lo: slo,
        hi: shi,
        step,
        p0: x0,
        p1: x1,
    };
    let bottom = top + row_h * ncat as f64;
    for t in sc.ticks() {
        let x = sc.at(t);
        c.d.line(
            x,
            top,
            x,
            bottom,
            &format!(
                r#"stroke="{}" stroke-width="1""#,
                if t.abs() < 1e-9 {
                    theme::AXIS
                } else {
                    theme::GRID
                }
            ),
        );
        c.d.text(
            x,
            bottom + size + 4.0,
            &c.fmt(t, dec),
            TextStyle::new(size, theme::FAINT).anchor(Anchor::Middle),
        );
    }
    let mut tops: Vec<Vec<(f64, f64)>> = vec![Vec::new(); spec.series.len()];
    for i in 0..ncat {
        let ry = top + row_h * i as f64;
        let ls = &labels[i];
        let lh = size * 1.2;
        let ty = ry + row_h / 2.0 - (ls.len() as f64 - 1.0) * lh / 2.0 + size * 0.35;
        c.d.lines(
            x0 - 10.0,
            ty,
            ls,
            lh,
            TextStyle::new(size, theme::INK).anchor(Anchor::End),
        );
        let (mut pos, mut neg) = (0.0f64, 0.0f64);
        let by0 = ry + (row_h - bar_h * nb as f64) / 2.0;
        for (si, v) in vals.iter().enumerate() {
            let v = v[i];
            let col = theme::series_color(&spec.colors, si);
            let (a, b, y) = if stacked {
                let r = if v >= 0.0 {
                    let r = (pos, pos + v);
                    pos += v;
                    r
                } else {
                    let r = (neg, neg + v);
                    neg += v;
                    r
                };
                (r.0, r.1, by0)
            } else {
                (0.0f64.max(slo), v, by0 + si as f64 * bar_h)
            };
            let (xa, xb) = (sc.at(a), sc.at(b));
            let inset = if nb > 1 { 1.0 } else { 0.0 };
            c.d.rect(
                xa.min(xb),
                y + inset,
                (xb - xa).abs(),
                bar_h - 2.0 * inset,
                2.0,
                &format!(r#"fill="{col}""#),
            );
            if stacked {
                c.d.line(
                    xa.max(xb),
                    y,
                    xa.max(xb),
                    y + bar_h,
                    r#"stroke="white" stroke-width="1""#,
                );
            }
            tops[si].push((xa.max(xb), y + bar_h / 2.0));
            if !stacked {
                let label = number(v, vdec, c.lang);
                let fs = c.m.tiny.min(bar_h + 1.0);
                c.d.text(
                    xa.max(xb) + 4.0,
                    y + bar_h / 2.0 + fs * 0.35,
                    &label,
                    TextStyle::new(fs, theme::INK),
                );
            }
        }
    }
    // Reference lines on the value axis; highlights.
    for a in &spec.annotations {
        match a {
            Annotation::RefLine { axis, value, label } if axis == "y" => {
                let x = sc.at(*value);
                c.d.line(
                    x,
                    top - c.m.tiny - 6.0,
                    x,
                    bottom,
                    &format!(
                        r#"stroke="{}" stroke-width="1.4" stroke-dasharray="6 4""#,
                        theme::MAROON
                    ),
                );
                let t = truncate(label, c.inner_w() * 0.4, c.m.tiny, false);
                let anchor = if x > (x0 + x1) / 2.0 {
                    Anchor::End
                } else {
                    Anchor::Start
                };
                pill_text(
                    &mut c.d,
                    if anchor == Anchor::End {
                        x - 3.0
                    } else {
                        x + 3.0
                    },
                    top - 5.0,
                    &t,
                    c.m.tiny,
                    theme::MAROON,
                    anchor,
                );
            }
            Annotation::Highlight { series, index, .. } => {
                if let Some(si) = spec.series.iter().position(|s| &s.name == series) {
                    if let Some(&(x, y)) = tops[si].get(*index) {
                        c.d.circle(
                            x,
                            y,
                            6.0,
                            &format!(r#"fill="none" stroke="{}" stroke-width="2""#, theme::MAROON),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    c.y = bottom + size + 10.0;
    if !cap.is_empty() {
        let t = truncate(&cap, x1 - x0, size, false);
        c.d.text(
            (x0 + x1) / 2.0,
            c.y + size,
            &t,
            c.style().anchor(Anchor::Middle),
        );
        c.y += size * 1.6;
    }
    if matches!(
        lpos,
        Some(LegendPosition::Bottom) | Some(LegendPosition::Right)
    ) {
        c.y += 4.0;
        let y0 = c.y;
        let h = legend_at(c, &items, y0);
        c.y += h;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------
// Donut
// ---------------------------------------------------------------------------------------------------------

fn arc_path(cx: f64, cy: f64, r0: f64, r1: f64, a0: f64, a1: f64) -> String {
    let large = if a1 - a0 > PI { 1 } else { 0 };
    let p = |r: f64, a: f64| (cx + r * a.cos(), cy + r * a.sin());
    let (ox0, oy0) = p(r1, a0);
    let (ox1, oy1) = p(r1, a1);
    let (ix1, iy1) = p(r0, a1);
    let (ix0, iy0) = p(r0, a0);
    format!(
        "M{} {}A{} {} 0 {} 1 {} {}L{} {}A{} {} 0 {} 0 {} {}Z",
        n(ox0),
        n(oy0),
        n(r1),
        n(r1),
        large,
        n(ox1),
        n(oy1),
        n(ix1),
        n(iy1),
        n(r0),
        n(r0),
        large,
        n(ix0),
        n(iy0)
    )
}

fn donut(c: &mut Ctx) -> Result<()> {
    let spec = c.spec;
    let s = &spec.series[0];
    let vals: Vec<f64> = (0..spec.categories.len())
        .map(|i| val(s, i).max(0.0))
        .collect();
    let total: f64 = vals.iter().sum();
    if total <= 0.0 {
        return Err(VisualError::Invalid(
            "a donut needs values above zero".into(),
        ));
    }
    let narrow = c.m.width < 500.0;
    let dia = if narrow { 170.0 } else { 200.0 };
    let r1 = dia / 2.0;
    let r0 = r1 * 0.6;
    let vdec = value_decimals(&vals);
    let top = c.y + 4.0;
    let (cx, cy) = if narrow {
        (c.m.width / 2.0, top + r1)
    } else {
        (c.m.pad + r1 + 10.0, top + r1)
    };
    let mut a = -PI / 2.0;
    for (i, v) in vals.iter().enumerate() {
        if *v <= 0.0 {
            continue;
        }
        let col = theme::series_color(&spec.colors, i);
        let sweep = v / total * 2.0 * PI;
        if sweep >= 2.0 * PI - 1e-9 {
            c.d.path(
                &arc_path(cx, cy, r0, r1, a, a + PI),
                &format!(r#"fill="{col}""#),
            );
            c.d.path(
                &arc_path(cx, cy, r0, r1, a + PI, a + 2.0 * PI),
                &format!(r#"fill="{col}""#),
            );
        } else {
            c.d.path(
                &arc_path(cx, cy, r0, r1, a, a + sweep),
                &format!(r#"fill="{col}" stroke="white" stroke-width="1.5""#),
            );
        }
        a += sweep;
    }
    // Centre: total and unit.
    let tot = number(total, vdec, c.lang);
    c.d.text(
        cx,
        cy + 2.0,
        &tot,
        TextStyle::new(c.m.title + 6.0, theme::INK)
            .bold()
            .anchor(Anchor::Middle),
    );
    let unit = spec
        .unit
        .clone()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| i18n::word("total", c.lang).to_string());
    let ut = truncate(&unit, r0 * 1.6, c.m.small, false);
    c.d.text(
        cx,
        cy + 2.0 + c.m.small * 1.5,
        &ut,
        c.style().anchor(Anchor::Middle),
    );

    // Legend with values and shares.
    let size = c.m.small;
    let row_h = size * 1.8;
    let rows: Vec<(String, String)> = spec
        .categories
        .iter()
        .zip(&vals)
        .map(|(cat, v)| {
            let pct = number(
                v / total * 100.0,
                if v / total * 100.0 < 10.0 { 1 } else { 0 },
                c.lang,
            );
            (
                cat.clone(),
                format!("{}  ({pct}%)", number(*v, vdec, c.lang)),
            )
        })
        .collect();
    let (lx, ly, lw) = if narrow {
        (c.m.pad, top + dia + 14.0, c.inner_w())
    } else {
        let lx = cx + r1 + 30.0;
        (
            lx,
            cy - rows.len() as f64 * row_h / 2.0,
            (c.m.width - c.m.pad - lx).min(320.0),
        )
    };
    let vw = rows
        .iter()
        .map(|(_, v)| width(v, size, false))
        .fold(0.0, f64::max);
    for (i, (cat, v)) in rows.iter().enumerate() {
        let y = ly + i as f64 * row_h + row_h / 2.0;
        let col = theme::series_color(&spec.colors, i);
        c.d.rect(lx, y - 5.0, 10.0, 10.0, 2.0, &format!(r#"fill="{col}""#));
        let name_w = lw - 18.0 - vw - 12.0;
        c.d.text_fit(
            lx + 18.0,
            y + size * 0.35,
            cat,
            name_w,
            TextStyle::new(size, theme::INK),
        );
        c.d.text(
            lx + lw,
            y + size * 0.35,
            v,
            TextStyle::new(size, theme::MUTED).anchor(Anchor::End),
        );
    }
    let legend_bottom = ly + rows.len() as f64 * row_h;
    c.y = (top + dia).max(legend_bottom) + 4.0;
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------
// Radar
// ---------------------------------------------------------------------------------------------------------

fn radar(c: &mut Ctx) -> Result<()> {
    let spec = c.spec;
    let k = spec.categories.len();
    let max = spec
        .series
        .iter()
        .flat_map(|s| s.values.iter().copied())
        .filter(|v| v.is_finite())
        .fold(0.0, f64::max);
    let (_, hi, step) = nice_scale(0.0, max, 4);
    let size = c.m.small;
    let narrow = c.m.width < 500.0;
    let label_w = if narrow { 84.0 } else { 120.0 };
    let r = if narrow { 100.0 } else { 120.0 };
    let top = c.y + size * 2.6 + 6.0;
    let cx = c.m.width / 2.0;
    let cy = top + r;
    let ang = |i: usize| -PI / 2.0 + 2.0 * PI * i as f64 / k as f64;
    // Rings.
    let rings = ((hi / step).round() as usize).max(1);
    for j in 1..=rings {
        let rr = r * j as f64 / rings as f64;
        let pts: Vec<String> = (0..k)
            .map(|i| {
                format!(
                    "{},{}",
                    n(cx + rr * ang(i).cos()),
                    n(cy + rr * ang(i).sin())
                )
            })
            .collect();
        c.d.raw(&format!(
            r#"<polygon points="{}" fill="none" stroke="{}" stroke-width="1"/>"#,
            pts.join(" "),
            theme::GRID
        ));
        let tv = number(step * j as f64, decimals_for(step), c.lang);
        c.d.text(
            cx + 3.0,
            cy - rr + c.m.tiny + 1.0,
            &tv,
            TextStyle::new(c.m.tiny, theme::FAINT),
        );
    }
    for (i, category) in spec.categories.iter().enumerate() {
        let (x, y) = (cx + r * ang(i).cos(), cy + r * ang(i).sin());
        c.d.line(
            cx,
            cy,
            x,
            y,
            &format!(r#"stroke="{}" stroke-width="1""#, theme::GRID),
        );
        let lines = wrap(category, label_w, size, false, 2);
        let (ca, sa) = (ang(i).cos(), ang(i).sin());
        let lx = cx + (r + 10.0) * ca;
        let ly = cy + (r + 10.0) * sa;
        let anchor = if ca.abs() < 0.2 {
            Anchor::Middle
        } else if ca > 0.0 {
            Anchor::Start
        } else {
            Anchor::End
        };
        let lh = size * 1.2;
        let block = lines.len() as f64 * lh;
        let y0 = if sa < -0.5 {
            ly - block + size * 0.8
        } else if sa > 0.5 {
            ly + size
        } else {
            ly - block / 2.0 + size * 0.85
        };
        c.d.lines(
            lx,
            y0,
            &lines,
            lh,
            TextStyle::new(size, theme::INK).anchor(anchor),
        );
    }
    for (si, s) in spec.series.iter().enumerate() {
        let col = theme::series_color(&spec.colors, si);
        let pts: Vec<String> = (0..k)
            .map(|i| {
                let rr = r * (val(s, i).max(0.0) / hi).min(1.0);
                format!(
                    "{},{}",
                    n(cx + rr * ang(i).cos()),
                    n(cy + rr * ang(i).sin())
                )
            })
            .collect();
        c.d.raw(&format!(
            r#"<polygon points="{}" fill="{col}" fill-opacity="0.14" stroke="{col}" stroke-width="2" stroke-linejoin="round"/>"#,
            pts.join(" ")
        ));
        for p in &pts {
            let (x, y) = p.split_once(',').unwrap();
            c.d.raw(&format!(
                r#"<circle cx="{x}" cy="{y}" r="2.6" fill="{col}"/>"#
            ));
        }
    }
    c.y = cy + r + size * 2.6 + 8.0;
    let items = series_items(spec, |_| true);
    if legend_pos(spec, items.len() > 1).is_some() {
        let y0 = c.y;
        let h = legend_at(c, &items, y0);
        c.y += h;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------
// Heatmap
// ---------------------------------------------------------------------------------------------------------

fn lerp_color(t: f64) -> String {
    // White to the accent red.
    let t = t.clamp(0.0, 1.0);
    let (r0, g0, b0) = (0xFB as f64, 0xF0 as f64, 0xF0 as f64);
    let (r1, g1, b1) = (0xC8 as f64, 0x00 as f64, 0x0A as f64);
    let m = |a: f64, b: f64| (a + (b - a) * t).round() as u8;
    format!("#{:02X}{:02X}{:02X}", m(r0, r1), m(g0, g1), m(b0, b1))
}

fn heatmap(c: &mut Ctx) -> Result<()> {
    let spec = c.spec;
    let ncol = spec.categories.len();
    let nrow = spec.series.len();
    let size = c.m.small;
    let all: Vec<f64> = spec
        .series
        .iter()
        .flat_map(|s| (0..ncol).map(move |i| val(s, i)))
        .collect();
    let (mn, mx) = all
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let vdec = value_decimals(&all);
    let col_max = c.inner_w() * 0.3;
    let rlabels: Vec<String> = spec
        .series
        .iter()
        .map(|s| truncate(&s.name, col_max, size, false))
        .collect();
    let col_w = rlabels
        .iter()
        .map(|l| width(l, size, false))
        .fold(0.0, f64::max);
    let x0 = c.m.pad + col_w + 8.0;
    let x1 = c.m.width - c.m.pad;
    let cw = (x1 - x0) / ncol as f64;
    let ch = (size * 2.2).max(22.0);
    let top = c.y + 4.0;
    for (ri, s) in spec.series.iter().enumerate() {
        let y = top + ri as f64 * ch;
        c.d.text(
            x0 - 8.0,
            y + ch / 2.0 + size * 0.35,
            &rlabels[ri],
            TextStyle::new(size, theme::INK).anchor(Anchor::End),
        );
        for ci in 0..ncol {
            let v = val(s, ci);
            let t = if mx > mn { (v - mn) / (mx - mn) } else { 0.5 };
            let x = x0 + ci as f64 * cw;
            c.d.rect(
                x + 1.0,
                y + 1.0,
                cw - 2.0,
                ch - 2.0,
                2.0,
                &format!(r#"fill="{}""#, lerp_color(t)),
            );
            let label = number(v, vdec, c.lang);
            if width(&label, c.m.tiny, false) <= cw - 4.0 {
                let fill = if t > 0.55 { "#FFFFFF" } else { theme::INK };
                c.d.text(
                    x + cw / 2.0,
                    y + ch / 2.0 + c.m.tiny * 0.35,
                    &label,
                    TextStyle::new(c.m.tiny, fill).anchor(Anchor::Middle),
                );
            }
        }
    }
    let bottom = top + nrow as f64 * ch;
    let centers: Vec<f64> = (0..ncol).map(|i| x0 + cw * (i as f64 + 0.5)).collect();
    let cl = layout_cat_labels(
        &spec.categories,
        cw,
        size,
        if c.m.width < 500.0 { 90.0 } else { 120.0 },
    );
    draw_cat_labels(c, &cl, &centers, bottom + 2.0);
    c.y = bottom + 2.0 + cl.height;
    x_caption(c, (x0 + x1) / 2.0);
    // Colour scale.
    c.d.def(r##"<linearGradient id="hm-scale" x1="0" x2="1" y1="0" y2="0"><stop offset="0" stop-color="#EEF3F9"/><stop offset="1" stop-color="#1F4E79"/></linearGradient>"##);
    let sw = 140.0f64.min(c.inner_w() * 0.4);
    let sy = c.y + 6.0;
    let lo_t = number(if mn.is_finite() { mn } else { 0.0 }, vdec, c.lang);
    let hi_t = number(if mx.is_finite() { mx } else { 0.0 }, vdec, c.lang);
    let lw = width(&lo_t, c.m.tiny, false);
    c.d.text(
        c.m.pad,
        sy + 9.0,
        &lo_t,
        TextStyle::new(c.m.tiny, theme::MUTED),
    );
    c.d.rect(
        c.m.pad + lw + 6.0,
        sy,
        sw,
        12.0,
        2.0,
        r#"fill="url(#hm-scale)""#,
    );
    c.d.text(
        c.m.pad + lw + sw + 12.0,
        sy + 9.0,
        &hi_t,
        TextStyle::new(c.m.tiny, theme::MUTED),
    );
    let cap = axis_caption(spec, None);
    if !cap.is_empty() {
        let after = c.m.pad + lw + sw + 18.0 + width(&hi_t, c.m.tiny, false);
        c.d.text_fit(
            after,
            sy + 9.0,
            &cap,
            x1 - after,
            TextStyle::new(c.m.tiny, theme::MUTED),
        );
    }
    c.y = sy + 16.0;
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------
// Scatter
// ---------------------------------------------------------------------------------------------------------

fn scatter(c: &mut Ctx) -> Result<()> {
    let spec = c.spec;
    let pts: Vec<&Point> = spec.series.iter().flat_map(|s| s.points.iter()).collect();
    if pts.is_empty() {
        return Err(VisualError::Invalid("a scatter chart needs points".into()));
    }
    let fold = |f: fn(&Point) -> f64| {
        pts.iter()
            .map(|p| f(p))
            .filter(|v| v.is_finite())
            .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)))
    };
    let (xmn, xmx) = fold(|p| p.x);
    let (ymn, ymx) = fold(|p| p.y);
    let (xlo, xhi, xstep) = nice_scale(
        xmn.min(0.0f64.max(xmn)),
        xmx,
        if c.m.width < 500.0 { 4 } else { 6 },
    );
    let (ylo, yhi, ystep) = nice_scale(ymn.min(0.0f64.max(ymn)), ymx, 5);
    let (xdec, ydec) = (decimals_for(xstep), decimals_for(ystep));
    let items = series_items(spec, |_| false);
    let lpos = legend_pos(spec, items.len() > 1);
    if lpos == Some(LegendPosition::Top) {
        let y0 = c.y;
        let h = legend_at(c, &items, y0);
        c.y += h + 6.0;
    }
    let cap = axis_caption(spec, None);
    if !cap.is_empty() {
        let t = truncate(&cap, c.inner_w(), c.m.small, false);
        c.d.text(c.m.pad, c.y + c.m.small, &t, c.style());
        c.y += c.m.small * 1.6;
    }
    let ys0 = Scale {
        lo: ylo,
        hi: yhi,
        step: ystep,
        p0: 0.0,
        p1: 1.0,
    };
    let tw = ys0
        .ticks()
        .iter()
        .map(|t| width(&number(*t, ydec, c.lang), c.m.small, false))
        .fold(0.0, f64::max);
    let x0 = c.m.pad + tw + 8.0;
    let x1 = c.m.width - c.m.pad - 8.0;
    let top = c.y + 6.0;
    let bottom = top + c.plot_h();
    let xs = Scale {
        lo: xlo,
        hi: xhi,
        step: xstep,
        p0: x0,
        p1: x1,
    };
    let ys = Scale {
        lo: ylo,
        hi: yhi,
        step: ystep,
        p0: bottom,
        p1: top,
    };
    for t in ys.ticks() {
        let y = ys.at(t);
        c.d.line(
            x0,
            y,
            x1,
            y,
            &format!(r#"stroke="{}" stroke-width="1""#, theme::GRID),
        );
        c.d.text(
            x0 - 6.0,
            y + c.m.small * 0.35,
            &number(t, ydec, c.lang),
            TextStyle::new(c.m.small, theme::FAINT).anchor(Anchor::End),
        );
    }
    for t in xs.ticks() {
        let x = xs.at(t);
        c.d.line(
            x,
            top,
            x,
            bottom,
            &format!(r#"stroke="{}" stroke-width="1""#, theme::GRID),
        );
        c.d.text(
            x,
            bottom + c.m.small + 4.0,
            &number(t, xdec, c.lang),
            TextStyle::new(c.m.small, theme::FAINT).anchor(Anchor::Middle),
        );
    }
    c.d.line(
        x0,
        bottom,
        x1,
        bottom,
        &format!(r#"stroke="{}" stroke-width="1""#, theme::AXIS),
    );
    c.d.line(
        x0,
        top,
        x0,
        bottom,
        &format!(r#"stroke="{}" stroke-width="1""#, theme::AXIS),
    );
    // Reference lines.
    for a in &spec.annotations {
        if let Annotation::RefLine { axis, value, label } = a {
            let t = truncate(label, (x1 - x0) * 0.5, c.m.tiny, false);
            if axis == "x" {
                let x = xs.at(*value);
                c.d.line(
                    x,
                    top,
                    x,
                    bottom,
                    &format!(
                        r#"stroke="{}" stroke-width="1.4" stroke-dasharray="6 4""#,
                        theme::MAROON
                    ),
                );
                pill_text(
                    &mut c.d,
                    x + 4.0,
                    top + c.m.tiny + 2.0,
                    &t,
                    c.m.tiny,
                    theme::MAROON,
                    Anchor::Start,
                );
            } else {
                let y = ys.at(*value);
                c.d.line(
                    x0,
                    y,
                    x1,
                    y,
                    &format!(
                        r#"stroke="{}" stroke-width="1.4" stroke-dasharray="6 4""#,
                        theme::MAROON
                    ),
                );
                pill_text(
                    &mut c.d,
                    x1 - 2.0,
                    y - 4.0,
                    &t,
                    c.m.tiny,
                    theme::MAROON,
                    Anchor::End,
                );
            }
        }
    }
    // Points, then labels where they do not collide.
    let mut boxes: Vec<BoxR> = Vec::new();
    for (si, s) in spec.series.iter().enumerate() {
        let col = theme::series_color(&spec.colors, si);
        for p in &s.points {
            let (x, y) = (xs.at(p.x), ys.at(p.y));
            c.d.circle(
                x,
                y,
                4.0,
                &format!(r#"fill="{col}" fill-opacity="0.85" stroke="white" stroke-width="1""#),
            );
            boxes.push(BoxR {
                x0: x - 4.0,
                y0: y - 4.0,
                x1: x + 4.0,
                y1: y + 4.0,
            });
        }
    }
    let tiny = c.m.tiny;
    for s in &spec.series {
        for p in &s.points {
            let Some(l) = p.label.as_deref().filter(|l| !l.trim().is_empty()) else {
                continue;
            };
            let l = truncate(l, 120.0, tiny, false);
            let (x, y) = (xs.at(p.x), ys.at(p.y));
            let tries = [
                (x + 7.0, y + tiny * 0.35, Anchor::Start),
                (x - 7.0, y + tiny * 0.35, Anchor::End),
                (x, y - 8.0, Anchor::Middle),
                (x, y + tiny + 6.0, Anchor::Middle),
            ];
            for (tx, ty, an) in tries {
                let b = text_box(tx, ty, &l, tiny, an);
                if b.x0 >= x0
                    && b.x1 <= x1
                    && b.y0 >= top
                    && b.y1 <= bottom
                    && !boxes.iter().any(|o| o.hits(&b))
                {
                    c.d.text(tx, ty, &l, TextStyle::new(tiny, theme::INK).anchor(an));
                    boxes.push(b);
                    break;
                }
            }
        }
    }
    c.y = bottom + c.m.small + 10.0;
    x_caption(c, (x0 + x1) / 2.0);
    if matches!(
        lpos,
        Some(LegendPosition::Bottom) | Some(LegendPosition::Right)
    ) {
        c.y += 4.0;
        let y0 = c.y;
        let h = legend_at(c, &items, y0);
        c.y += h;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------
// Gantt and milestones
// ---------------------------------------------------------------------------------------------------------

/// Label every k-th time unit so the numbers do not touch.
fn tick_every(unit_w: f64, label_w: f64) -> usize {
    for k in [1usize, 2, 4, 5, 10, 13, 26, 52] {
        if unit_w * k as f64 >= label_w + 6.0 {
            return k;
        }
    }
    100
}

/// Greedy label rows: each label box (x0, x1) goes on the first level it does not overlap.
fn assign_levels(spans: &[(f64, f64)]) -> Vec<usize> {
    let mut ends: Vec<f64> = Vec::new();
    spans
        .iter()
        .map(|(a, b)| {
            for (lvl, e) in ends.iter_mut().enumerate() {
                if *a >= *e + 6.0 {
                    *e = *b;
                    return lvl;
                }
            }
            ends.push(*b);
            ends.len() - 1
        })
        .collect()
}

fn gantt(c: &mut Ctx) -> Result<()> {
    let spec = c.spec;
    if spec.tasks.is_empty() {
        return Err(VisualError::Invalid("a timeline needs tasks".into()));
    }
    let tu = spec.time_unit.unwrap_or_default();
    let size = c.m.small;
    let end = spec
        .tasks
        .iter()
        .map(|t| t.start.max(0.0) + t.length.max(0.0))
        .chain(spec.milestones.iter().map(|m| m.at))
        .filter(|v| v.is_finite())
        .fold(1.0, f64::max);
    let span = end.ceil().max(1.0);
    // Groups: colour per group in order of first appearance.
    let mut groups: Vec<String> = Vec::new();
    for t in &spec.tasks {
        if let Some(g) = t.group.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
            if !groups.iter().any(|x| x == g) {
                groups.push(g.to_string());
            }
        }
    }
    let gcolor = |g: Option<&str>| -> String {
        match g
            .map(str::trim)
            .and_then(|g| groups.iter().position(|x| x == g))
        {
            Some(i) => theme::series_color(&spec.colors, i),
            None => theme::series_color(&spec.colors, 0),
        }
    };
    let col_max = c.inner_w() * if c.m.width < 500.0 { 0.4 } else { 0.32 };
    let names: Vec<Vec<String>> = spec
        .tasks
        .iter()
        .map(|t| wrap(&t.name, col_max, size, false, 2))
        .collect();
    let col_w = names
        .iter()
        .map(|l| text::max_width(l, size, false))
        .fold(0.0, f64::max)
        .min(col_max)
        .max(40.0);
    let two = names.iter().any(|l| l.len() > 1);
    let row_h = if two {
        size * 2.6 + 6.0
    } else {
        size * 1.4 + 12.0
    };
    let x0 = c.m.pad + col_w + 12.0;
    let x1 = c.m.width - c.m.pad - 4.0;
    let uw = (x1 - x0) / span;

    // Header: unit word and numbers.
    let header_y = c.y + size;
    c.d.text(
        x0 - 12.0,
        header_y,
        i18n::unit_title(tu, c.lang),
        TextStyle::new(size, theme::MUTED).anchor(Anchor::End),
    );
    let every = tick_every(uw, width(&number(span, 0, c.lang), size, false));
    let top = header_y + 8.0;
    let rows_bottom = top + row_h * spec.tasks.len() as f64;
    for i in 0..span as usize {
        if i % every == 0 {
            let cx = x0 + uw * (i as f64 + 0.5);
            let label = number((i + 1) as f64, 0, c.lang);
            if every == 1 || cx + width(&label, size, false) / 2.0 <= x1 {
                c.d.text(
                    cx,
                    header_y,
                    &label,
                    TextStyle::new(size, theme::MUTED).anchor(Anchor::Middle),
                );
            }
        }
    }
    // Row bands and grid.
    for i in 0..spec.tasks.len() {
        if i % 2 == 0 {
            c.d.rect(
                c.m.pad,
                top + row_h * i as f64,
                c.inner_w(),
                row_h,
                0.0,
                r##"fill="#F6F7F9""##,
            );
        }
    }
    for i in 0..=span as usize {
        if i % every == 0 || i == span as usize {
            let x = x0 + uw * i as f64;
            c.d.line(
                x,
                top,
                x,
                rows_bottom,
                &format!(r#"stroke="{}" stroke-width="1""#, theme::GRID),
            );
        }
    }
    // Tasks.
    let bar_h = (row_h * 0.46).clamp(12.0, 18.0);
    for (i, t) in spec.tasks.iter().enumerate() {
        let ry = top + row_h * i as f64;
        let lines = &names[i];
        let lh = size * 1.2;
        let ty = ry + row_h / 2.0 - (lines.len() as f64 - 1.0) * lh / 2.0 + size * 0.35;
        c.d.lines(
            c.m.pad + 2.0,
            ty,
            lines,
            lh,
            TextStyle::new(size, theme::INK),
        );
        let bx = x0 + uw * t.start.max(0.0);
        let bw = (uw * t.length.max(0.0)).max(3.0);
        let col = gcolor(t.group.as_deref());
        c.d.rect(
            bx,
            ry + (row_h - bar_h) / 2.0,
            bw,
            bar_h,
            3.0,
            &format!(r#"fill="{col}""#),
        );
        // Duration inside the bar, or after it, when it fits.
        let dur = i18n::duration(t.length, tu, c.lang);
        let dw = width(&dur, c.m.tiny, false);
        let by = ry + row_h / 2.0 + c.m.tiny * 0.35;
        if dw + 8.0 <= bw {
            c.d.text(
                bx + bw / 2.0,
                by,
                &dur,
                TextStyle::new(c.m.tiny, "#FFFFFF").anchor(Anchor::Middle),
            );
        } else if bx + bw + 4.0 + dw <= x1 {
            c.d.text(
                bx + bw + 4.0,
                by,
                &dur,
                TextStyle::new(c.m.tiny, theme::MUTED),
            );
        }
    }
    // Vertical reference lines (x = time).
    for a in &spec.annotations {
        if let Annotation::RefLine { axis, value, label } = a {
            if axis == "x" {
                let x = (x0 + uw * value).clamp(x0, x1);
                c.d.line(
                    x,
                    top,
                    x,
                    rows_bottom,
                    &format!(
                        r#"stroke="{}" stroke-width="1.4" stroke-dasharray="6 4""#,
                        theme::MAROON
                    ),
                );
                let t = truncate(label, 140.0, c.m.tiny, false);
                let anchor = if x > (x0 + x1) / 2.0 {
                    Anchor::End
                } else {
                    Anchor::Start
                };
                pill_text(
                    &mut c.d,
                    if anchor == Anchor::End {
                        x - 3.0
                    } else {
                        x + 3.0
                    },
                    top + c.m.tiny + 1.0,
                    &t,
                    c.m.tiny,
                    theme::MAROON,
                    anchor,
                );
            }
        }
    }
    c.d.line(
        c.m.pad,
        rows_bottom,
        x1,
        rows_bottom,
        &format!(r#"stroke="{}" stroke-width="1""#, theme::AXIS),
    );
    c.y = rows_bottom;
    // Milestones: diamond on the axis, dashed line through the rows, labels in non-overlapping rows.
    if !spec.milestones.is_empty() {
        let mut ms: Vec<&Milestone> = spec.milestones.iter().collect();
        ms.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
        let tiny = c.m.small;
        let max_lw = if c.m.width < 500.0 { 110.0 } else { 150.0 };
        let labels: Vec<String> = ms
            .iter()
            .map(|m| truncate(&m.name, max_lw, tiny, false))
            .collect();
        let spans: Vec<(f64, f64, f64)> = ms
            .iter()
            .zip(&labels)
            .map(|(m, l)| {
                let x = (x0 + uw * m.at).clamp(x0, x1);
                let w = width(l, tiny, false);
                let a = (x - w / 2.0).clamp(c.m.pad, c.m.width - c.m.pad - w);
                (x, a, a + w)
            })
            .collect();
        let levels = assign_levels(&spans.iter().map(|(_, a, b)| (*a, *b)).collect::<Vec<_>>());
        let dy = rows_bottom + 10.0;
        for (k, (x, a, _)) in spans.iter().enumerate() {
            c.d.line(
                *x,
                top,
                *x,
                dy,
                &format!(
                    r#"stroke="{}" stroke-width="1" stroke-dasharray="3 3""#,
                    theme::INK
                ),
            );
            c.d.path(
                &format!("M{} {}l6 6l-6 6l-6 -6z", n(*x), n(dy - 6.0)),
                &format!(r#"fill="{}""#, theme::INK),
            );
            let ly = dy + 8.0 + tiny + levels[k] as f64 * tiny * 1.4;
            c.d.text(*a, ly, &labels[k], TextStyle::new(tiny, theme::INK).bold());
        }
        let nlev = levels.iter().max().map_or(1, |m| m + 1);
        c.y = dy + 8.0 + nlev as f64 * tiny * 1.4 + 2.0;
    }
    c.y += 6.0;
    if groups.len() > 1 && legend_pos(spec, true).is_some() {
        let items: Vec<Item> = groups
            .iter()
            .map(|g| Item {
                label: g.clone(),
                color: gcolor(Some(g)),
                line: false,
            })
            .collect();
        let y0 = c.y;
        let h = legend_at(c, &items, y0);
        c.y += h;
    }
    Ok(())
}

fn milestones(c: &mut Ctx) -> Result<()> {
    let spec = c.spec;
    if spec.milestones.is_empty() {
        return Err(VisualError::Invalid(
            "a milestone strip needs milestones".into(),
        ));
    }
    let tu = spec.time_unit.unwrap_or_default();
    let size = c.m.small;
    let mut ms: Vec<&Milestone> = spec.milestones.iter().collect();
    ms.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
    let span = ms
        .iter()
        .map(|m| m.at)
        .filter(|v| v.is_finite())
        .fold(1.0, f64::max)
        .ceil();
    let x0 = c.m.pad + 10.0;
    let x1 = c.m.width - c.m.pad - 10.0;
    let uw = (x1 - x0) / span;
    let max_lw = if c.m.width < 500.0 { 100.0 } else { 140.0 };
    // Labels: name (up to 2 lines) and "Week 3".
    let blocks: Vec<(Vec<String>, String)> = ms
        .iter()
        .map(|m| {
            let mut lines = wrap(&m.name, max_lw, size, true, 2);
            if lines.is_empty() {
                lines.push(String::new());
            }
            let when = format!(
                "{} {}",
                i18n::unit_title(tu, c.lang),
                number(m.at, 1, c.lang)
            );
            (lines, when)
        })
        .collect();
    let spans: Vec<(f64, f64, f64)> = ms
        .iter()
        .zip(&blocks)
        .map(|(m, (lines, when))| {
            let x = x0 + uw * m.at;
            let w = text::max_width(lines, size, true).max(width(when, c.m.tiny, false));
            let a = (x - w / 2.0).clamp(c.m.pad, c.m.width - c.m.pad - w);
            (x, a, a + w)
        })
        .collect();
    // Alternate above / below; each side gets its own greedy levels.
    let mut above = Vec::new();
    let mut below = Vec::new();
    for (i, s) in spans.iter().enumerate() {
        if i % 2 == 0 {
            above.push((i, (s.1, s.2)));
        } else {
            below.push((i, (s.1, s.2)));
        }
    }
    let la = assign_levels(&above.iter().map(|x| x.1).collect::<Vec<_>>());
    let lb = assign_levels(&below.iter().map(|x| x.1).collect::<Vec<_>>());
    let block_h = size * 1.25 * 2.0 + c.m.tiny * 1.3 + 8.0;
    let na = la.iter().max().map_or(0, |m| m + 1);
    let nb = lb.iter().max().map_or(0, |m| m + 1);
    let axis_y = c.y + na as f64 * block_h + 14.0;
    // Track.
    c.d.line(
        x0,
        axis_y,
        x1,
        axis_y,
        &format!(
            r#"stroke="{}" stroke-width="3" stroke-linecap="round""#,
            theme::AXIS
        ),
    );
    let mm = c.m;
    let draw_block = |d: &mut Doc, i: usize, level: usize, up: bool| {
        let (x, a, _) = spans[i];
        let (lines, when) = &blocks[i];
        let lh = size * 1.25;
        let h = lines.len() as f64 * lh + mm.tiny * 1.3;
        let (ty, stem_end) = if up {
            let bottom_y = axis_y - 14.0 - level as f64 * block_h;
            (bottom_y - h + size, bottom_y + 2.0)
        } else {
            let top_y = axis_y + 14.0 + level as f64 * block_h;
            (top_y + size, top_y - size + 2.0)
        };
        d.line(
            x,
            axis_y,
            x,
            stem_end,
            &format!(r#"stroke="{}" stroke-width="1""#, theme::AXIS),
        );
        d.lines(a, ty, lines, lh, TextStyle::new(size, theme::INK).bold());
        d.text(
            a,
            ty + lines.len() as f64 * lh - 1.0,
            when,
            TextStyle::new(mm.tiny, theme::MUTED),
        );
    };
    for (k, (i, _)) in above.iter().enumerate() {
        draw_block(&mut c.d, *i, la[k], true);
    }
    for (k, (i, _)) in below.iter().enumerate() {
        draw_block(&mut c.d, *i, lb[k], false);
    }
    for (i, (x, _, _)) in spans.iter().enumerate() {
        let col = theme::series_color(&spec.colors, if i + 1 == spans.len() { 0 } else { 1 });
        c.d.path(
            &format!("M{} {}l7 7l-7 7l-7 -7z", n(*x), n(axis_y - 7.0)),
            &format!(r#"fill="{col}" stroke="white" stroke-width="1.5""#),
        );
    }
    c.y = axis_y + 8.0 + nb as f64 * block_h + 6.0;
    Ok(())
}
