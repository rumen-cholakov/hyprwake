//! Layout inference: the split tree a workspace was in, read back from where
//! its tiled windows sit, and the steps that rebuild it.
//!
//! Capture records each window's rectangle. A dwindle workspace is a binary
//! tree of splits, so the rectangles can be cut apart again: find a straight
//! line no window crosses, split the windows on either side of it, recurse.
//! Each cut gives a split's axis and ratio.
//!
//! Inference has to be able to say "I do not know", because a wrong tree
//! replayed looks worse than no tree at all. Every anomaly lowers a
//! confidence number instead of producing a guess: a gap that is not the
//! configured one, a hole no tiling would leave (a window smaller than its
//! tile, as a pseudotiled one is), a split the compositor would not keep.
//! Restore replays a tree only above a threshold, per workspace, and falls
//! back to launching in reading order otherwise.
//!
//! Replay, measured against Hyprland 0.56.2 in a nested compositor:
//!
//! - `layoutmsg preselect r|d` decides which side of the focused window the
//!   next one opens on. It forces the split's orientation only while
//!   `dwindle:preserve_split` is on; with it off, Hyprland re-derives every
//!   orientation from the shape of the box on each recalculation. A tree
//!   captured under that setting is shaped that way already, and one that
//!   is not is scored as unreachable.
//! - `layoutmsg splitratio X exact` sets the split that is the focused
//!   window's parent, the first child taking `X / 2`. Set right after a
//!   split is made, while both its children are single windows, every split
//!   in the tree can be reached.
//! - Without a preselection, a new window's side follows the cursor, so every
//!   split is preselected.

use crate::hyprctl::HyprctlClient;
use crate::session::{Axis, LayoutNode};

/// A window's rectangle, in layout pixels: `at` and `size` as hyprctl reports
/// them, which exclude the border.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(at: [i32; 2], size: [i32; 2]) -> Self {
        Rect {
            x: at[0],
            y: at[1],
            w: size[0],
            h: size[1],
        }
    }

    fn start(&self, axis: Axis) -> i32 {
        match axis {
            Axis::Horizontal => self.x,
            Axis::Vertical => self.y,
        }
    }

    fn len(&self, axis: Axis) -> i32 {
        match axis {
            Axis::Horizontal => self.w,
            Axis::Vertical => self.h,
        }
    }

    fn end(&self, axis: Axis) -> i32 {
        self.start(axis) + self.len(axis)
    }

    fn bbox(rects: impl Iterator<Item = Rect>) -> Option<Rect> {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        let mut any = false;
        for r in rects {
            any = true;
            x0 = x0.min(r.x);
            y0 = y0.min(r.y);
            x1 = x1.max(r.x + r.w);
            y1 = y1.max(r.y + r.h);
        }
        any.then(|| Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }

    /// Split at `at` along `axis`, the two halves sharing the line.
    fn cut(&self, axis: Axis, at: i32) -> (Rect, Rect) {
        match axis {
            Axis::Horizontal => (
                Rect {
                    w: at - self.x,
                    ..*self
                },
                Rect {
                    x: at,
                    w: self.x + self.w - at,
                    ..*self
                },
            ),
            Axis::Vertical => (
                Rect {
                    h: at - self.y,
                    ..*self
                },
                Rect {
                    y: at,
                    h: self.y + self.h - at,
                    ..*self
                },
            ),
        }
    }
}

fn other(axis: Axis) -> Axis {
    match axis {
        Axis::Horizontal => Axis::Vertical,
        Axis::Vertical => Axis::Horizontal,
    }
}

/// The compositor settings inference needs, read once per capture.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutOptions {
    /// `general:layout`: `dwindle`, `master`, or something not inferred.
    pub layout: String,
    /// Space between two tiled windows: an inner gap and a border on each.
    pub gap: i32,
    pub preserve_split: bool,
    /// `dwindle:split_width_multiplier`: a box splits side by side when its
    /// width exceeds its height times this.
    pub split_width_multiplier: f32,
    pub master_orientation: String,
    pub master_new_status: String,
    pub master_new_on_top: bool,
}

impl Default for LayoutOptions {
    /// Hyprland's own defaults.
    fn default() -> Self {
        LayoutOptions {
            layout: "dwindle".to_string(),
            gap: 2 * 5 + 2,
            preserve_split: false,
            split_width_multiplier: 1.0,
            master_orientation: "left".to_string(),
            master_new_status: "slave".to_string(),
            master_new_on_top: false,
        }
    }
}

impl LayoutOptions {
    /// What the running compositor is configured with, defaults where a
    /// value cannot be read.
    pub fn read(hyprctl: &dyn HyprctlClient) -> Self {
        let d = LayoutOptions::default();
        let num = |name: &str| hyprctl.get_option(name).and_then(|v| option_number(&v));
        let gaps_in = num("general.gaps_in").unwrap_or(5.0);
        let border = num("general.border_size").unwrap_or(1.0);
        let text = |name: &str, default: &str| {
            hyprctl
                .get_option(name)
                .and_then(|v| v.get("str").and_then(|s| s.as_str()).map(String::from))
                .unwrap_or_else(|| default.to_string())
        };
        let flag = |name: &str, default: bool| {
            hyprctl
                .get_option(name)
                .and_then(|v| {
                    v.get("bool")
                        .and_then(|b| b.as_bool())
                        .or_else(|| v.get("int").and_then(|i| i.as_i64()).map(|i| i != 0))
                })
                .unwrap_or(default)
        };
        LayoutOptions {
            layout: text("general.layout", &d.layout),
            gap: (2.0 * gaps_in + 2.0 * border).round() as i32,
            preserve_split: flag("dwindle.preserve_split", d.preserve_split),
            split_width_multiplier: num("dwindle.split_width_multiplier")
                .map(|v| v as f32)
                .unwrap_or(d.split_width_multiplier),
            master_orientation: text("master.orientation", &d.master_orientation),
            master_new_status: text("master.new_status", &d.master_new_status),
            master_new_on_top: flag("master.new_on_top", d.master_new_on_top),
        }
    }

    /// The layout kind recorded in a snapshot, or `None` for a layout that
    /// is not inferred (scrolling, center-master, a plugin's).
    pub fn kind(&self) -> Option<String> {
        match (self.layout.as_str(), self.master_orientation.as_str()) {
            ("dwindle", _) => Some("dwindle".to_string()),
            ("master", o @ ("left" | "right" | "top" | "bottom")) => Some(format!("master-{o}")),
            _ => None,
        }
    }

    /// The orientation dwindle gives a box of this shape when nothing
    /// overrides it.
    fn natural(&self, r: Rect) -> Axis {
        if r.w as f32 > r.h as f32 * self.split_width_multiplier {
            Axis::Horizontal
        } else {
            Axis::Vertical
        }
    }
}

/// A number from a `getoption` reply: `int`, `float`, or the first value of
/// a `css` quadruple such as `"5 5 5 5"`.
fn option_number(v: &serde_json::Value) -> Option<f64> {
    v.get("int")
        .and_then(|i| i.as_f64())
        .or_else(|| v.get("float").and_then(|f| f.as_f64()))
        .or_else(|| {
            v.get("css")
                .and_then(|c| c.as_str())
                .and_then(|c| c.split_whitespace().next())
                .and_then(|n| n.parse().ok())
        })
}

/// Pixels of slack in every comparison: rounding in the compositor, and
/// windows that size themselves in character cells.
const SLACK: i32 = 4;

/// Below this a split is not one dwindle can make (`splitratio` is clamped
/// to 0.1–1.9, a first child of 5–95%).
const MIN_FRACTION: f32 = 0.05;

/// A tree read back from geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct Inferred {
    pub tree: LayoutNode,
    pub kind: String,
    /// 1.0 for a clean tiling; each anomaly multiplies it down.
    pub confidence: f32,
    /// What lowered the confidence, for the log and `--verbose`.
    pub notes: Vec<String>,
}

/// Infer the tree over one workspace's tiled windows: `(client index,
/// rectangle)` pairs, the index being the window's place in the snapshot.
///
/// `None` when the layout kind is not one inference understands, or when no
/// straight line separates the windows somewhere — an arrangement no split
/// tree produces.
pub fn infer(windows: &[(usize, Rect)], opts: &LayoutOptions) -> Option<Inferred> {
    let kind = opts.kind()?;
    let root = Rect::bbox(windows.iter().map(|(_, r)| *r))?;
    let mut notes = Vec::new();
    let (tree, mut confidence) = split(windows, root, opts, &mut notes)?;
    if let Some(side) = kind.strip_prefix("master-") {
        confidence *= master_shape(&tree, side, opts, &mut notes);
    }
    Some(Inferred {
        tree,
        kind,
        confidence,
        notes,
    })
}

fn split(
    windows: &[(usize, Rect)],
    tile: Rect,
    opts: &LayoutOptions,
    notes: &mut Vec<String>,
) -> Option<(LayoutNode, f32)> {
    if let [(client, rect)] = windows {
        // A window fills its tile but for half a gap on each inner side.
        let fits = (tile.w - rect.w).abs() <= opts.gap + SLACK
            && (tile.h - rect.h).abs() <= opts.gap + SLACK;
        let quality = if fits {
            1.0
        } else {
            notes.push(format!(
                "a window is {}x{} in a {}x{} tile, which no tiling leaves",
                rect.w, rect.h, tile.w, tile.h
            ));
            0.5
        };
        return Some((LayoutNode::Leaf { client: *client }, quality));
    }

    // Every clean line is a candidate, dwindle's own orientation for this
    // shape first. Where several would do — three columns are A|(B|C) or
    // (A|B)|C — the most plausible tree wins, ties going to the earlier
    // line: dwindle nests new windows inward, so the outermost split is
    // usually the oldest.
    let natural = opts.natural(tile);
    let mut best: Option<(LayoutNode, f32, Vec<String>)> = None;
    for axis in [natural, other(natural)] {
        for (first, second) in cuts(windows, axis) {
            let mut local = Vec::new();
            let Some((tree, quality)) =
                split_at(axis, &first, &second, tile, natural, opts, &mut local)
            else {
                continue;
            };
            if best.as_ref().is_none_or(|(_, q, _)| quality > *q) {
                let perfect = quality >= 1.0;
                best = Some((tree, quality, local));
                if perfect {
                    break;
                }
            }
        }
        if best.as_ref().is_some_and(|(_, q, _)| *q >= 1.0) {
            break;
        }
    }
    let (tree, quality, local) = best?;
    notes.extend(local);
    Some((tree, quality))
}

/// The split made by one candidate line, scored, with its two sides
/// inferred in turn.
fn split_at(
    axis: Axis,
    first: &[(usize, Rect)],
    second: &[(usize, Rect)],
    tile: Rect,
    natural: Axis,
    opts: &LayoutOptions,
    notes: &mut Vec<String>,
) -> Option<(LayoutNode, f32)> {
    let first_end = first.iter().map(|(_, r)| r.end(axis)).max()?;
    let second_start = second.iter().map(|(_, r)| r.start(axis)).min()?;
    let line = (first_end + second_start) / 2;
    let fraction = (line - tile.start(axis)) as f32 / tile.len(axis).max(1) as f32;
    let (a_tile, b_tile) = tile.cut(axis, line);

    let mut quality = 1.0;
    let gap = second_start - first_end;
    if (gap - opts.gap).abs() > SLACK {
        notes.push(format!(
            "a {gap}px gap between windows where {}px was configured",
            opts.gap
        ));
        quality *= 0.6;
    }
    let across = other(axis);
    for (group, half) in [(first, a_tile), (second, b_tile)] {
        let Some(span) = Rect::bbox(group.iter().map(|(_, r)| *r)) else {
            continue;
        };
        if (half.len(across) - span.len(across)).abs() > opts.gap + SLACK {
            notes.push("windows that do not fill the space beside them".to_string());
            quality *= 0.5;
        }
    }
    if !(MIN_FRACTION..=1.0 - MIN_FRACTION).contains(&fraction) {
        notes.push(format!(
            "a {:.0}% split, beyond what dwindle makes",
            fraction * 100.0
        ));
        quality *= 0.5;
    }
    if !opts.preserve_split && opts.layout == "dwindle" && axis != natural {
        notes.push(
            "a split across its box's shape, which Hyprland re-derives while \
             dwindle:preserve_split is off"
                .to_string(),
        );
        quality *= 0.3;
    }

    let (a, qa) = split(first, a_tile, opts, notes)?;
    let (b, qb) = split(second, b_tile, opts, notes)?;
    Some((
        LayoutNode::Split {
            axis,
            ratio: fraction,
            a: Box::new(a),
            b: Box::new(b),
        },
        quality * qa * qb,
    ))
}

type Group = Vec<(usize, Rect)>;

/// Past this many windows only the first clean line on each axis is
/// considered, so a crowded workspace cannot make inference explode.
const SEARCH_LIMIT: usize = 10;

/// Every line along `axis` that no window crosses, as the windows before it
/// and the windows after, nearest the start first.
fn cuts(windows: &[(usize, Rect)], axis: Axis) -> Vec<(Group, Group)> {
    let mut sorted: Vec<(usize, Rect)> = windows.to_vec();
    sorted.sort_by_key(|(i, r)| (r.start(axis), *i));
    let mut out = Vec::new();
    let mut reach = i32::MIN;
    for k in 1..sorted.len() {
        reach = reach.max(sorted[k - 1].1.end(axis));
        if reach <= sorted[k].1.start(axis) + SLACK {
            let (a, b) = sorted.split_at(k);
            out.push((a.to_vec(), b.to_vec()));
            if windows.len() > SEARCH_LIMIT {
                break;
            }
        }
    }
    out
}

/// How well a tree fits the master layout: masters on the configured side,
/// and each side one row or column.
fn master_shape(
    tree: &LayoutNode,
    side: &str,
    opts: &LayoutOptions,
    notes: &mut Vec<String>,
) -> f32 {
    let mut quality = 1.0;
    if opts.master_new_status != "slave" || opts.master_new_on_top {
        notes.push(format!(
            "master:new_status is {} and new_on_top {}; replay assumes slave and false",
            opts.master_new_status, opts.master_new_on_top
        ));
        quality *= 0.5;
    }
    let LayoutNode::Split { axis, a, b, .. } = tree else {
        return quality;
    };
    let want = match side {
        "left" | "right" => Axis::Horizontal,
        _ => Axis::Vertical,
    };
    if *axis != want {
        notes.push(format!("masters are not on the {side}"));
        return quality * 0.3;
    }
    for half in [a, b] {
        if !is_line(half, other(want)) {
            notes.push("a master or stack side that is not one row or column".to_string());
            quality *= 0.5;
        }
    }
    quality
}

fn is_line(node: &LayoutNode, axis: Axis) -> bool {
    match node {
        LayoutNode::Leaf { .. } => true,
        LayoutNode::Split {
            axis: a,
            a: x,
            b: y,
            ..
        } => *a == axis && is_line(x, axis) && is_line(y, axis),
    }
}

/// Every window in the tree, in order.
pub fn leaves(node: &LayoutNode) -> Vec<usize> {
    match node {
        LayoutNode::Leaf { client } => vec![*client],
        LayoutNode::Split { a, b, .. } => {
            let mut out = leaves(a);
            out.extend(leaves(b));
            out
        }
    }
}

fn first_leaf(node: &LayoutNode) -> usize {
    match node {
        LayoutNode::Leaf { client } => *client,
        LayoutNode::Split { a, .. } => first_leaf(a),
    }
}

/// One step of rebuilding a tree.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplayOp {
    /// Launch this window and wait until it maps.
    Spawn(usize),
    /// Focus this window, then send the layout message.
    Message { focus: usize, message: String },
}

/// The steps that rebuild `tree` on an empty workspace.
pub fn replay(tree: &LayoutNode, kind: &str) -> Vec<ReplayOp> {
    let mut ops = Vec::new();
    match kind.strip_prefix("master-") {
        None => {
            ops.push(ReplayOp::Spawn(first_leaf(tree)));
            dwindle(tree, &mut ops);
        }
        Some(side) => master(tree, side, &mut ops),
    }
    ops
}

/// Each split: focus its first window, preselect the side the second opens
/// on, launch the second, and set the ratio while both are single windows.
fn dwindle(node: &LayoutNode, ops: &mut Vec<ReplayOp>) {
    let LayoutNode::Split { axis, ratio, a, b } = node else {
        return;
    };
    let (first, second) = (first_leaf(a), first_leaf(b));
    let direction = match axis {
        Axis::Horizontal => "r",
        Axis::Vertical => "d",
    };
    ops.push(ReplayOp::Message {
        focus: first,
        message: format!("preselect {direction}"),
    });
    ops.push(ReplayOp::Spawn(second));
    ops.push(ReplayOp::Message {
        focus: first,
        message: format!("splitratio {:.3} exact", (ratio * 2.0).clamp(0.1, 1.9)),
    });
    dwindle(a, ops);
    dwindle(b, ops);
}

/// Masters first, then the stack, in order — the layout appends each new
/// window to the stack — then the extra masters promoted, the orientation and
/// the master's share set.
fn master(tree: &LayoutNode, side: &str, ops: &mut Vec<ReplayOp>) {
    let (masters, stack, fraction) = match tree {
        LayoutNode::Leaf { client } => (vec![*client], vec![], None),
        LayoutNode::Split { ratio, a, b, .. } => match side {
            "left" | "top" => (leaves(a), leaves(b), Some(*ratio)),
            _ => (leaves(b), leaves(a), Some(1.0 - *ratio)),
        },
    };
    for &client in masters.iter().chain(&stack) {
        ops.push(ReplayOp::Spawn(client));
    }
    let lead = masters[0];
    for &extra in &masters[1..] {
        ops.push(ReplayOp::Message {
            focus: extra,
            message: "addmaster".to_string(),
        });
    }
    ops.push(ReplayOp::Message {
        focus: lead,
        message: format!("orientation{side}"),
    });
    if let Some(f) = fraction.filter(|_| !stack.is_empty()) {
        ops.push(ReplayOp::Message {
            focus: lead,
            message: format!("mfact exact {:.3}", f.clamp(0.05, 0.95)),
        });
    }
}

/// Whether two trees describe the same arrangement: the same splits over the
/// same windows, ratios within `tolerance`.
pub fn same(a: &LayoutNode, b: &LayoutNode, tolerance: f32) -> bool {
    match (a, b) {
        (LayoutNode::Leaf { client: x }, LayoutNode::Leaf { client: y }) => x == y,
        (
            LayoutNode::Split {
                axis: ax,
                ratio: ra,
                a: a1,
                b: b1,
            },
            LayoutNode::Split {
                axis: bx,
                ratio: rb,
                a: a2,
                b: b2,
            },
        ) => {
            ax == bx
                && (ra - rb).abs() <= tolerance
                && same(a1, a2, tolerance)
                && same(b1, b2, tolerance)
        }
        _ => false,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The rectangles a tree tiles `tile` into: each window inset by half a
    /// gap on every side that faces another window, flush on the outside.
    pub(crate) fn render(
        node: &LayoutNode,
        tile: Rect,
        outer: Rect,
        gap: i32,
    ) -> Vec<(usize, Rect)> {
        match node {
            LayoutNode::Leaf { client } => {
                let half = gap / 2;
                let inset = |at_outer: bool| if at_outer { 0 } else { half };
                let left = inset(tile.x == outer.x);
                let top = inset(tile.y == outer.y);
                let right = inset(tile.x + tile.w == outer.x + outer.w);
                let bottom = inset(tile.y + tile.h == outer.y + outer.h);
                vec![(
                    *client,
                    Rect {
                        x: tile.x + left,
                        y: tile.y + top,
                        w: tile.w - left - right,
                        h: tile.h - top - bottom,
                    },
                )]
            }
            LayoutNode::Split { axis, ratio, a, b } => {
                let line = tile.start(*axis) + (tile.len(*axis) as f32 * ratio).round() as i32;
                let (ta, tb) = tile.cut(*axis, line);
                let mut out = render(a, ta, outer, gap);
                out.extend(render(b, tb, outer, gap));
                out
            }
        }
    }

    pub(crate) fn leaf(client: usize) -> LayoutNode {
        LayoutNode::Leaf { client }
    }

    pub(crate) fn node(axis: Axis, ratio: f32, a: LayoutNode, b: LayoutNode) -> LayoutNode {
        LayoutNode::Split {
            axis,
            ratio,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    const SCREEN: Rect = Rect {
        x: 21,
        y: 21,
        w: 1878,
        h: 1038,
    };

    fn opts() -> LayoutOptions {
        LayoutOptions::default()
    }

    #[test]
    fn real_geometry_from_a_nested_compositor_is_read_back() {
        // Recorded in a nested Hyprland 0.56.2 with default gaps: A over
        // C|D on the left at splitratio 0.5 and 1.4, B alone on the right.
        let windows = vec![
            (0, Rect::new([21, 21], [933, 253])),
            (1, Rect::new([966, 21], [933, 1038])),
            (2, Rect::new([21, 286], [651, 773])),
            (3, Rect::new([684, 286], [270, 773])),
        ];
        let got = infer(&windows, &opts()).unwrap();
        assert!(got.confidence > 0.99, "{got:?}");
        let LayoutNode::Split { axis, ratio, a, b } = &got.tree else {
            panic!("{got:?}")
        };
        assert_eq!(*axis, Axis::Horizontal);
        assert!((ratio - 0.5).abs() < 0.01);
        assert_eq!(**b, leaf(1));
        let LayoutNode::Split {
            axis,
            ratio,
            a: top,
            b: bottom,
        } = &**a
        else {
            panic!()
        };
        assert_eq!(*axis, Axis::Vertical);
        assert!((ratio - 0.25).abs() < 0.02, "{ratio}");
        assert_eq!(**top, leaf(0));
        let LayoutNode::Split { ratio, .. } = &**bottom else {
            panic!()
        };
        assert!((ratio - 0.70).abs() < 0.02, "{ratio}");
    }

    /// This machine's settings: Omarchy's 1px inner gaps and 2px borders,
    /// with preserve_split on.
    fn this_machine() -> LayoutOptions {
        LayoutOptions {
            gap: 6, // two 1px inner gaps, two 2px borders
            preserve_split: true,
            ..opts()
        }
    }

    #[test]
    fn this_machines_real_workspaces_are_read_back() {
        // Recorded from the development machine (eDP-1, 1920x1080 at 1.25):
        // one terminal on the left, two stacked on the right.
        let ws3 = vec![
            (1, Rect::new([771, 30], [761, 412])),
            (2, Rect::new([771, 448], [761, 412])),
            (7, Rect::new([4, 30], [761, 830])),
        ];
        let got = infer(&ws3, &this_machine()).unwrap();
        assert!(got.confidence > 0.99, "{got:?}");
        assert_eq!(
            got.tree,
            node(
                Axis::Horizontal,
                0.5,
                leaf(7),
                node(Axis::Vertical, 0.5, leaf(1), leaf(2))
            ),
            "both splits even, to the pixel"
        );
    }

    #[test]
    fn a_tile_held_by_an_ignored_window_is_a_hole() {
        // The same machine with a nested compositor's window — an ignored
        // class, so not saved — tiled under the left terminal. The saved
        // windows no longer fill the workspace, and the tree cannot be
        // rebuilt without the window that is missing from it.
        let ws1 = vec![
            (4, Rect::new([771, 30], [761, 830])),
            (6, Rect::new([4, 30], [761, 412])),
        ];
        let got = infer(&ws1, &this_machine()).unwrap();
        assert!(got.confidence < 0.7, "{got:?}");
    }

    #[test]
    fn a_window_smaller_than_its_tile_lowers_confidence_below_the_default_bar() {
        // B is pseudotiled: it keeps its own size inside its tile.
        let windows = vec![
            (0, Rect::new([21, 21], [933, 1038])),
            (1, Rect::new([1200, 300], [400, 300])),
        ];
        let got = infer(&windows, &opts()).unwrap();
        assert!(got.confidence < 0.7, "{got:?}");
        assert!(
            got.notes.iter().any(|n| n.contains("tile")),
            "{:?}",
            got.notes
        );
    }

    #[test]
    fn overlapping_windows_have_no_tree() {
        let windows = vec![
            (0, Rect::new([21, 21], [1000, 1038])),
            (1, Rect::new([900, 21], [999, 1038])),
        ];
        assert_eq!(infer(&windows, &opts()), None);
    }

    #[test]
    fn a_split_against_its_shape_is_unreachable_without_preserve_split() {
        // Two windows stacked in a wide box: dwindle would put them side by
        // side the moment it recalculated.
        let tree = node(Axis::Vertical, 0.5, leaf(0), leaf(1));
        let windows = render(&tree, SCREEN, SCREEN, 12);
        let off = infer(&windows, &opts()).unwrap();
        assert!(off.confidence < 0.7, "{off:?}");
        let on = infer(
            &windows,
            &LayoutOptions {
                preserve_split: true,
                ..opts()
            },
        )
        .unwrap();
        assert!(on.confidence > 0.99, "{on:?}");
    }

    #[test]
    fn a_layout_kind_inference_does_not_understand_is_left_alone() {
        let o = LayoutOptions {
            layout: "scrolling".into(),
            ..opts()
        };
        assert_eq!(infer(&[(0, SCREEN)], &o), None);
        let center = LayoutOptions {
            layout: "master".into(),
            master_orientation: "center".into(),
            ..opts()
        };
        assert_eq!(center.kind(), None);
    }

    #[test]
    fn options_are_read_from_getoption_replies() {
        let mut hypr = crate::hyprctl::mock::MockHyprctl::default();
        for (k, v) in [
            ("general.gaps_in", serde_json::json!({"css": "8 8 8 8"})),
            ("general.border_size", serde_json::json!({"int": 2})),
            ("dwindle.preserve_split", serde_json::json!({"bool": true})),
            ("general.layout", serde_json::json!({"str": "master"})),
            ("master.orientation", serde_json::json!({"str": "right"})),
        ] {
            hypr.options.insert(k.to_string(), v);
        }
        let o = LayoutOptions::read(&hypr);
        assert_eq!(o.gap, 20);
        assert!(o.preserve_split);
        assert_eq!(o.kind().as_deref(), Some("master-right"));
        assert_eq!(
            LayoutOptions::read(&crate::hyprctl::mock::MockHyprctl::default()),
            LayoutOptions::default(),
            "unreadable options are Hyprland's defaults"
        );
    }

    /// xorshift: deterministic, so a failure reproduces.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// A random tree of `n` windows whose splits each go the way dwindle
    /// would take a box of that shape, so it is reachable either way.
    fn random_tree(
        rng: &mut Rng,
        clients: &mut std::ops::Range<usize>,
        n: usize,
        tile: Rect,
    ) -> LayoutNode {
        if n == 1 {
            return leaf(clients.next().unwrap());
        }
        let axis = opts().natural(tile);
        let ratio = 0.25 + rng.below(51) as f32 / 100.0; // 0.25..0.75
        let left = 1 + rng.below(n as u64 - 1) as usize;
        let line = tile.start(axis) + (tile.len(axis) as f32 * ratio).round() as i32;
        let (ta, tb) = tile.cut(axis, line);
        let a = random_tree(rng, clients, left, ta);
        let b = random_tree(rng, clients, n - left, tb);
        node(axis, ratio, a, b)
    }

    #[test]
    fn random_trees_round_trip_through_their_geometry() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for case in 0..300 {
            let n = 1 + rng.below(6) as usize;
            let tree = random_tree(&mut rng, &mut (0..n), n, SCREEN);
            let windows = render(&tree, SCREEN, SCREEN, 12);
            let got = infer(&windows, &opts())
                .unwrap_or_else(|| panic!("case {case}: no tree for {tree:?}"));
            // Equivalent trees may differ in shape where the geometry cannot
            // tell (three columns); what must hold is that the inferred one
            // tiles the screen into the same rectangles.
            let mut again = render(&got.tree, SCREEN, SCREEN, 12);
            let mut want = windows.clone();
            again.sort_by_key(|(i, _)| *i);
            want.sort_by_key(|(i, _)| *i);
            for ((i, r1), (_, r2)) in want.iter().zip(&again) {
                let off = (r1.x - r2.x).abs()
                    + (r1.y - r2.y).abs()
                    + (r1.w - r2.w).abs()
                    + (r1.h - r2.h).abs();
                assert!(
                    off <= 4,
                    "case {case}: window {i} moved {off}px\n{tree:?}\n{:?}",
                    got.tree
                );
            }
            assert!(got.confidence > 0.99, "case {case}: {got:?}");
            assert_eq!(leaves(&got.tree).len(), n);
        }
    }

    #[test]
    fn a_dwindle_tree_replays_split_by_split() {
        // A | (B over C)
        let tree = node(
            Axis::Horizontal,
            0.6,
            leaf(0),
            node(Axis::Vertical, 0.3, leaf(1), leaf(2)),
        );
        assert_eq!(
            replay(&tree, "dwindle"),
            vec![
                ReplayOp::Spawn(0),
                ReplayOp::Message {
                    focus: 0,
                    message: "preselect r".into()
                },
                ReplayOp::Spawn(1),
                ReplayOp::Message {
                    focus: 0,
                    message: "splitratio 1.200 exact".into()
                },
                ReplayOp::Message {
                    focus: 1,
                    message: "preselect d".into()
                },
                ReplayOp::Spawn(2),
                ReplayOp::Message {
                    focus: 1,
                    message: "splitratio 0.600 exact".into()
                },
            ]
        );
    }

    #[test]
    fn a_master_tree_replays_masters_then_stack() {
        // Masters 0 and 1 on the right, stack 2 and 3 on the left.
        let tree = node(
            Axis::Horizontal,
            0.4,
            node(Axis::Vertical, 0.5, leaf(2), leaf(3)),
            node(Axis::Vertical, 0.5, leaf(0), leaf(1)),
        );
        assert_eq!(
            replay(&tree, "master-right"),
            vec![
                ReplayOp::Spawn(0),
                ReplayOp::Spawn(1),
                ReplayOp::Spawn(2),
                ReplayOp::Spawn(3),
                ReplayOp::Message {
                    focus: 1,
                    message: "addmaster".into()
                },
                ReplayOp::Message {
                    focus: 0,
                    message: "orientationright".into()
                },
                ReplayOp::Message {
                    focus: 0,
                    message: "mfact exact 0.600".into()
                },
            ]
        );
    }

    #[test]
    fn a_master_layout_is_inferred_with_its_side() {
        let o = LayoutOptions {
            layout: "master".into(),
            ..opts()
        };
        // Recorded: one master at mfact 0.55 on the left, two in the stack.
        let windows = vec![
            (0, Rect::new([21, 21], [1027, 1038])),
            (1, Rect::new([1060, 21], [839, 513])),
            (2, Rect::new([1060, 546], [839, 513])),
        ];
        let got = infer(&windows, &o).unwrap();
        assert_eq!(got.kind, "master-left");
        assert!(got.confidence > 0.99, "{got:?}");
        let ops = replay(&got.tree, &got.kind);
        assert_eq!(
            &ops[..3],
            &[ReplayOp::Spawn(0), ReplayOp::Spawn(1), ReplayOp::Spawn(2)]
        );
        let ReplayOp::Message { message, .. } = ops.last().unwrap() else {
            panic!()
        };
        assert!(message.starts_with("mfact exact 0.55"), "{message}");
    }

    #[test]
    fn trees_compare_by_shape_windows_and_ratio() {
        let a = node(Axis::Horizontal, 0.5, leaf(0), leaf(1));
        assert!(same(
            &a,
            &node(Axis::Horizontal, 0.52, leaf(0), leaf(1)),
            0.05
        ));
        assert!(!same(
            &a,
            &node(Axis::Horizontal, 0.6, leaf(0), leaf(1)),
            0.05
        ));
        assert!(!same(
            &a,
            &node(Axis::Vertical, 0.5, leaf(0), leaf(1)),
            0.05
        ));
        assert!(!same(
            &a,
            &node(Axis::Horizontal, 0.5, leaf(1), leaf(0)),
            0.05
        ));
    }
}
