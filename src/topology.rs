//! Which attached monitor a saved monitor's workspaces belong on.
//!
//! A connector name is where a display is plugged in, not which display it
//! is: `DP-1` on one dock is not `DP-1` on another, and the same panel on a
//! different port gets a different name. A snapshot records each monitor's
//! make, model and serial for that reason, and this module uses them — but
//! only when asked to. With the default `[monitors] missing =
//! "preserve-numbers"` matching is by connector name, exactly as before 0.2.0,
//! because nothing multi-monitor is allowed to change behaviour until the
//! release gate has exercised it.

use crate::config::{MissingMonitor, MonitorConfig};
use crate::hyprctl::HyprMonitor;
use crate::session::Monitor;
use serde::Serialize;

/// How a saved monitor was recognised among the attached ones, strongest
/// first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MonitorMatch {
    Serial,
    MakeModel,
    Name,
    Resolution,
}

impl MonitorMatch {
    fn describe(&self) -> &'static str {
        match self {
            MonitorMatch::Serial => "its serial",
            MonitorMatch::MakeModel => "its make and model",
            MonitorMatch::Name => "its connector",
            MonitorMatch::Resolution => "its resolution alone",
        }
    }
}

/// Why a missing monitor's workspaces went somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instead {
    /// `[monitors.remap]` named the monitor to use.
    Configured,
    /// `missing = "collapse"`: onto the focused monitor.
    Collapsed,
    /// `missing = "remap"`: onto the most similar monitor.
    Nearest,
}

/// Where a saved monitor's workspaces go.
#[derive(Debug, Clone, Copy)]
pub enum Placement<'a> {
    /// The same display is attached.
    Found {
        live: &'a HyprMonitor,
        how: MonitorMatch,
    },
    /// It is not; another was chosen.
    Instead { live: &'a HyprMonitor, why: Instead },
    /// It is not, and its workspaces are left where Hyprland puts them.
    Missing,
}

impl<'a> Placement<'a> {
    pub fn live(&self) -> Option<&'a HyprMonitor> {
        match self {
            Placement::Found { live, .. } | Placement::Instead { live, .. } => Some(live),
            Placement::Missing => None,
        }
    }

    /// Said in the report whenever this is not the plain case — the same
    /// connector, still attached — so a move nobody expected explains itself.
    pub fn reason(&self, saved: &str) -> Option<String> {
        match self {
            Placement::Found {
                how: MonitorMatch::Name,
                live,
            } if live.name == saved => None,
            Placement::Found { live, how } => Some(format!(
                "{saved} is now {}, recognised by {}",
                live.name,
                how.describe()
            )),
            Placement::Instead { live, why } => Some(match why {
                Instead::Configured => {
                    format!("{saved} is gone; [monitors.remap] says {}", live.name)
                }
                Instead::Collapsed => {
                    format!(
                        "{saved} is gone; collapsed onto the focused monitor {}",
                        live.name
                    )
                }
                Instead::Nearest => format!(
                    "{saved} is gone; {} is the nearest in shape and size",
                    live.name
                ),
            }),
            Placement::Missing => None,
        }
    }
}

/// Decide where `saved`'s workspaces go.
///
/// `record` is the snapshot's description of that monitor, when it has one.
/// `by_identity` switches from connector matching to the identity ladder; it
/// is on for every `missing` strategy but the default, and for
/// `--topology saved`.
pub fn place<'a>(
    saved: &str,
    record: Option<&Monitor>,
    live: &'a [HyprMonitor],
    config: &MonitorConfig,
    by_identity: bool,
) -> Placement<'a> {
    let found = if by_identity {
        identify(saved, record, live)
    } else {
        live.iter()
            .find(|l| l.name == saved)
            .map(|l| (l, MonitorMatch::Name))
    };
    if let Some((live, how)) = found {
        return Placement::Found { live, how };
    }

    if let Some(target) = config.remap.get(saved) {
        if let Some(live) = live.iter().find(|l| &l.name == target) {
            return Placement::Instead {
                live,
                why: Instead::Configured,
            };
        }
    }

    match config.missing {
        MissingMonitor::PreserveNumbers => Placement::Missing,
        MissingMonitor::Collapse => match focused(live) {
            Some(live) => Placement::Instead {
                live,
                why: Instead::Collapsed,
            },
            None => Placement::Missing,
        },
        MissingMonitor::Remap => {
            let nearest = match record {
                Some(r) => nearest(r, live),
                None => focused(live),
            };
            match nearest {
                Some(live) => Placement::Instead {
                    live,
                    why: Instead::Nearest,
                },
                None => Placement::Missing,
            }
        }
    }
}

/// The attached monitor that is the same display as `saved`, if any.
///
/// Serial first — unique when present, though built-in panels often report
/// none. Then make and model, when exactly one attached monitor has them. Then
/// the connector, unless the panel there is visibly a different one: dock B's
/// `DP-1` is not dock A's. Then resolution, when exactly one monitor has it.
pub fn identify<'a>(
    saved: &str,
    record: Option<&Monitor>,
    live: &'a [HyprMonitor],
) -> Option<(&'a HyprMonitor, MonitorMatch)> {
    if let Some(r) = record {
        if !r.serial.is_empty() {
            if let Some(l) = live.iter().find(|l| l.serial == r.serial) {
                return Some((l, MonitorMatch::Serial));
            }
        }
        if !r.make.is_empty() && !r.model.is_empty() {
            let same: Vec<&HyprMonitor> = live
                .iter()
                .filter(|l| l.make == r.make && l.model == r.model)
                .collect();
            if same.len() == 1 {
                return Some((same[0], MonitorMatch::MakeModel));
            }
        }
    }

    if let Some(l) = live.iter().find(|l| l.name == saved) {
        let contradicts = record.is_some_and(|r| {
            !r.make.is_empty() && !l.make.is_empty() && (r.make != l.make || r.model != l.model)
        });
        if !contradicts {
            return Some((l, MonitorMatch::Name));
        }
    }

    let r = record?;
    let same: Vec<&HyprMonitor> = live
        .iter()
        .filter(|l| l.width == r.width && l.height == r.height)
        .collect();
    (same.len() == 1).then(|| (same[0], MonitorMatch::Resolution))
}

/// Saved monitors that `--topology saved` would need and cannot find.
pub fn absent(session_monitors: &[Monitor], live: &[HyprMonitor]) -> Vec<String> {
    session_monitors
        .iter()
        .filter(|m| identify(&m.name, Some(m), live).is_none())
        .map(|m| m.name.clone())
        .collect()
}

fn focused(live: &[HyprMonitor]) -> Option<&HyprMonitor> {
    live.iter().find(|l| l.focused).or_else(|| live.first())
}

/// Nearest in aspect ratio, then in area, then by name for stability.
fn nearest<'a>(saved: &Monitor, live: &'a [HyprMonitor]) -> Option<&'a HyprMonitor> {
    let (sw, sh) = logical(saved.width, saved.height, saved.scale, saved.transform);
    let aspect = sw / sh;
    live.iter().min_by(|a, b| {
        let key = |m: &HyprMonitor| {
            let (w, h) = logical(m.width, m.height, m.scale, m.transform);
            ((w / h - aspect).abs(), (w * h - sw * sh).abs())
        };
        let (ka, kb) = (key(a), key(b));
        ka.0.total_cmp(&kb.0)
            .then(ka.1.total_cmp(&kb.1))
            .then(a.name.cmp(&b.name))
    })
}

/// A monitor's size in layout coordinates: pixels divided by scale, with
/// width and height swapped when it is rotated a quarter turn.
fn logical(width: u32, height: u32, scale: f32, transform: u32) -> (f64, f64) {
    let scale = if scale > 0.0 { scale as f64 } else { 1.0 };
    let (w, h) = (width as f64 / scale, height as f64 / scale);
    let (w, h) = if transform % 2 == 1 { (h, w) } else { (w, h) };
    (w.max(1.0), h.max(1.0))
}

/// Carry a floating window from the monitor it was saved on to `to`,
/// keeping its relative position and its shape, and clamp it inside `to` so
/// it can never come back off screen.
///
/// Position scales per axis — a window in the top-right corner stays in the
/// top-right corner. Size scales by the *smaller* of the two ratios, so a
/// window keeps its aspect ratio: scaling each axis on its own squashes a
/// landscape window into a strip on a portrait monitor.
pub fn fit(at: [i32; 2], size: [i32; 2], from: &Monitor, to: &HyprMonitor) -> ([i32; 2], [i32; 2]) {
    let (fw, fh) = logical(from.width, from.height, from.scale, from.transform);
    let (tw, th) = logical(to.width, to.height, to.scale, to.transform);
    let (sx, sy) = (tw / fw, th / fh);
    let s = sx.min(sy);
    let w = ((size[0] as f64 * s).round() as i32).clamp(1, tw as i32);
    let h = ((size[1] as f64 * s).round() as i32).clamp(1, th as i32);
    let rel_x = (at[0] - from.position[0]) as f64 * sx;
    let rel_y = (at[1] - from.position[1]) as f64 * sy;
    let x = (to.x + rel_x.round() as i32).clamp(to.x, to.x + tw as i32 - w);
    let y = (to.y + rel_y.round() as i32).clamp(to.y, to.y + th as i32 - h);
    ([x, y], [w, h])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // The laptop fixture is `hyprctl monitors -j` recorded on the development
    // machine. The docked ones add a synthetic external display to that exact
    // record: there is no second display there to record.
    pub(crate) fn monitors(fixture: &str) -> Vec<HyprMonitor> {
        let text = match fixture {
            "laptop" => include_str!("../tests/fixtures/monitors-laptop.json"),
            "docked" => include_str!("../tests/fixtures/monitors-docked.json"),
            "docked-elsewhere" => include_str!("../tests/fixtures/monitors-docked-elsewhere.json"),
            "other-dock" => include_str!("../tests/fixtures/monitors-other-dock.json"),
            _ => unreachable!(),
        };
        serde_json::from_str(text).unwrap()
    }

    /// What a snapshot records about a monitor, as capture writes it.
    pub(crate) fn saved(m: &HyprMonitor) -> Monitor {
        Monitor {
            name: m.name.clone(),
            width: m.width,
            height: m.height,
            transform: m.transform,
            active_workspace: m.active_workspace.clone(),
            focused: m.focused,
            make: m.make.clone(),
            model: m.model.clone(),
            serial: m.serial.clone(),
            scale: m.scale,
            position: [m.x, m.y],
        }
    }

    fn dell() -> Monitor {
        saved(&monitors("docked")[1])
    }

    fn panel() -> Monitor {
        saved(&monitors("docked")[0])
    }

    #[test]
    fn the_real_panel_reports_no_serial() {
        // Why serial cannot be the only rung: the built-in panel this was
        // developed on has none.
        let laptop = monitors("laptop");
        assert_eq!(laptop[0].serial, "");
        assert!(!laptop[0].make.is_empty());
    }

    #[test]
    fn the_same_display_on_another_port_is_found_by_serial() {
        let live = monitors("docked-elsewhere");
        let (m, how) = identify("DP-1", Some(&dell()), &live).unwrap();
        assert_eq!((m.name.as_str(), how), ("DP-2", MonitorMatch::Serial));
    }

    #[test]
    fn a_panel_without_a_serial_is_found_by_make_and_model() {
        let live = monitors("docked");
        let (m, how) = identify("eDP-1", Some(&panel()), &live).unwrap();
        assert_eq!((m.name.as_str(), how), ("eDP-1", MonitorMatch::MakeModel));
    }

    #[test]
    fn a_different_panel_on_the_same_connector_is_not_the_same_monitor() {
        let live = monitors("other-dock");
        let (m, how) = identify("DP-1", Some(&dell()), &live).unwrap();
        assert_eq!(m.name, "DP-1");
        assert_eq!(
            how,
            MonitorMatch::Resolution,
            "the LG is only as good as a resolution match, never 'the same connector'"
        );
    }

    #[test]
    fn a_0_1_snapshot_monitor_is_matched_by_connector() {
        // No make, model or serial were recorded before 0.2.0.
        let mut old = dell();
        old.make.clear();
        old.model.clear();
        old.serial.clear();
        let live = monitors("other-dock");
        let (m, how) = identify("DP-1", Some(&old), &live).unwrap();
        assert_eq!((m.name.as_str(), how), ("DP-1", MonitorMatch::Name));
    }

    #[test]
    fn by_default_only_the_connector_counts() {
        let live = monitors("docked-elsewhere");
        let p = place(
            "DP-1",
            Some(&dell()),
            &live,
            &MonitorConfig::default(),
            false,
        );
        assert!(
            matches!(p, Placement::Missing),
            "0.1.x did not follow the Dell to DP-2"
        );
        assert_eq!(p.reason("DP-1"), None);
    }

    #[test]
    fn a_configured_remap_applies_even_by_default() {
        let config = MonitorConfig {
            remap: [("DP-1".to_string(), "eDP-1".to_string())].into(),
            ..Default::default()
        };
        let live = monitors("laptop");
        let p = place("DP-1", Some(&dell()), &live, &config, false);
        let Placement::Instead { live, why } = p else {
            panic!("{p:?}");
        };
        assert_eq!((live.name.as_str(), why), ("eDP-1", Instead::Configured));
        assert!(p.reason("DP-1").unwrap().contains("[monitors.remap]"));
    }

    #[test]
    fn collapse_and_remap_both_land_a_lost_monitor_somewhere() {
        let live = monitors("laptop");
        for (missing, why) in [
            (MissingMonitor::Collapse, Instead::Collapsed),
            (MissingMonitor::Remap, Instead::Nearest),
        ] {
            let config = MonitorConfig {
                missing,
                ..Default::default()
            };
            let p = place("DP-1", Some(&dell()), &live, &config, true);
            let Placement::Instead { live, why: got } = p else {
                panic!("{p:?}");
            };
            assert_eq!((live.name.as_str(), got), ("eDP-1", why));
        }
    }

    #[test]
    fn remap_picks_the_most_similar_monitor() {
        // Two candidates: the 16:9 panel and a portrait display. A 16:9
        // external goes to the 16:9 panel.
        let mut live = monitors("laptop");
        let mut portrait = monitors("other-dock")[1].clone();
        portrait.name = "DP-3".into();
        portrait.transform = 1;
        live.push(portrait);
        let config = MonitorConfig {
            missing: MissingMonitor::Remap,
            ..Default::default()
        };
        let mut gone = dell();
        gone.serial = "not-here".into();
        gone.make = "Nobody".into();
        gone.width = 3840;
        gone.height = 2160;
        let p = place("DP-9", Some(&gone), &live, &config, true);
        assert_eq!(p.live().unwrap().name, "eDP-1");
    }

    #[test]
    fn topology_saved_needs_every_saved_monitor() {
        let session = [panel(), dell()];
        assert_eq!(absent(&session, &monitors("laptop")), vec!["DP-1"]);
        assert!(absent(&session, &monitors("docked-elsewhere")).is_empty());
        assert!(absent(&session, &monitors("docked")).is_empty());
    }

    #[test]
    fn a_window_carried_to_a_smaller_monitor_keeps_its_place_and_stays_on_it() {
        let to = &monitors("laptop")[0]; // 1536x864 logical at 0,0
                                         // Right half of the Dell, which sits at x=1536.
        let (at, size) = fit([1536 + 1280, 100], [1200, 800], &dell(), to);
        assert_eq!(at, [768, 60], "same relative position");
        assert_eq!(size, [720, 480], "scaled by 1536/2560 and 864/1440");
    }

    #[test]
    fn a_window_carried_to_a_portrait_monitor_keeps_its_shape() {
        let mut portrait = monitors("laptop")[0].clone();
        portrait.transform = 1; // 864x1536 logical
        let (_, size) = fit([1536 + 100, 100], [1600, 900], &dell(), &portrait);
        let (before, after) = (1600.0 / 900.0, size[0] as f64 / size[1] as f64);
        assert!(
            (before - after).abs() < 0.01,
            "16:9 in, {size:?} out: squashed"
        );
        assert!(size[0] <= 864, "{size:?} still fits the width");
    }

    #[test]
    fn a_fitted_window_never_leaves_the_monitor() {
        let to = &monitors("laptop")[0];
        let (tw, th) = (1536, 864);
        for (at, size) in [
            ([1536 + 2500, 1400], [1000, 1000]),
            ([1536, 0], [4000, 3000]),
            ([-500, -500], [100, 100]),
            ([1536 + 2559, 1439], [1, 1]),
        ] {
            let (at, size) = fit(at, size, &dell(), to);
            assert!(at[0] >= to.x && at[1] >= to.y, "{at:?}");
            assert!(at[0] + size[0] <= to.x + tw, "{at:?} {size:?}");
            assert!(at[1] + size[1] <= to.y + th, "{at:?} {size:?}");
        }
    }
}
