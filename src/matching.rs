//! Which saved window a live one is.
//!
//! One ranking, used everywhere a live window is paired with a saved entry:
//! the restore sweep, `--missing-only` deciding what is already open, and
//! `hyprwake diff`. They used to answer the question three different ways,
//! so diff could call a window missing that restore would have recognised.
//!
//! Candidates are ranked, highest first, by:
//!
//! 1. **class** — the window's class is the saved class; otherwise the class
//!    it announced when it first mapped matches. Applications that rename
//!    themselves after mapping, and XWayland windows, are found by the
//!    second. Nothing else ever makes a match: a candidate needs one of them.
//! 2. **workspace** — the window is already where the entry was saved, so
//!    the pairing needs no correction.
//! 3. **title** — the saved title and the live one share their text. Only
//!    ever orders candidates that are equal on everything above, and never
//!    rejects one: among windows of one class it is the only thing that
//!    tells two VS Code windows apart, since they share a process, an
//!    executable and an initial title. A title the program has not set yet
//!    shares nothing and so changes nothing.
//! 4. **saved order**, which is what remains when nothing above separates
//!    candidates. Such a pairing is reported as a guess.

use crate::hyprctl::HyprClient;
use crate::session::SessionClient;
use std::cmp::Ordering;
use std::fmt;

/// How a live window was paired with a saved entry, and how sure that is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Basis {
    pub by: MatchedBy,
    pub same_workspace: bool,
    pub title: TitleMatch,
    /// Other candidates exactly as good as the one taken. Non-zero means the
    /// choice between them was saved order: a guess.
    pub tied: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchedBy {
    /// Only the class announced at first map agreed.
    InitialClass,
    Class,
}

/// How much of a saved title a live one shares, compared by its parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TitleMatch {
    None,
    /// This many title parts in common ("main.rs - hyprwake - Visual Studio
    /// Code" and "lib.rs - hyprwake - Visual Studio Code" share two).
    Parts(u8),
    Exact,
}

/// The best saved entry in `pending` for `window`, as an index into it.
pub fn best(pending: &[&SessionClient], window: &HyprClient) -> Option<(usize, Basis)> {
    let mut found: Option<(usize, Rank)> = None;
    let mut tied = 0;
    for (i, saved) in pending.iter().enumerate() {
        let Some(rank) = rank(saved, window) else {
            continue;
        };
        match found.as_ref().map(|(_, best)| rank.cmp(best)) {
            None | Some(Ordering::Greater) => {
                found = Some((i, rank));
                tied = 0;
            }
            Some(Ordering::Equal) => tied += 1,
            Some(Ordering::Less) => {}
        }
    }
    found.map(|(i, rank)| {
        (
            i,
            Basis {
                by: rank.by,
                same_workspace: rank.same_workspace,
                title: rank.title,
                tied,
            },
        )
    })
}

/// Pair the windows already on a desktop with saved entries, each used at
/// most once.
///
/// Returns, per live window, the index into `saved` it was paired with.
/// The sweep pairs windows one at a time as they arrive; a desktop that
/// already exists has no arrival order, so the strongest pairings are made
/// first. Otherwise a stray window could take the entry saved on workspace 1
/// before the window actually sitting on workspace 1 was looked at. Ties go
/// to the earlier window, then the earlier entry.
pub fn pair_all(saved: &[&SessionClient], live: &[&HyprClient]) -> Vec<Option<(usize, Basis)>> {
    let mut candidates: Vec<(Rank, usize, usize)> = Vec::new();
    for (w, window) in live.iter().enumerate() {
        for (s, entry) in saved.iter().enumerate() {
            if let Some(rank) = rank(entry, window) {
                candidates.push((rank, w, s));
            }
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

    let mut out: Vec<Option<(usize, Basis)>> = vec![None; live.len()];
    let mut taken = vec![false; saved.len()];
    for &(rank, w, s) in &candidates {
        if out[w].is_some() || taken[s] {
            continue;
        }
        let tied = candidates
            .iter()
            .filter(|&&(r, w2, s2)| w2 == w && s2 != s && r == rank && !taken[s2])
            .count();
        taken[s] = true;
        out[w] = Some((
            s,
            Basis {
                by: rank.by,
                same_workspace: rank.same_workspace,
                title: rank.title,
                tied,
            },
        ));
    }
    out
}

/// Ordered so that a greater `Rank` is a better candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Rank {
    by: MatchedBy,
    same_workspace: bool,
    title: TitleMatch,
}

fn rank(saved: &SessionClient, window: &HyprClient) -> Option<Rank> {
    let by = if saved.class == window.class {
        MatchedBy::Class
    } else if !window.initial_class.is_empty()
        && (saved.initial_class == window.initial_class
            || saved.class == window.initial_class
            || (!saved.initial_class.is_empty() && saved.initial_class == window.class))
    {
        MatchedBy::InitialClass
    } else {
        return None;
    };
    Some(Rank {
        by,
        same_workspace: saved.workspace.same(&window.workspace),
        title: title_match(&saved.title, &window.title),
    })
}

/// Separators programs put between the parts of a window title.
const TITLE_SEPARATORS: [&str; 5] = [" - ", " — ", " – ", " | ", " · "];

fn title_match(saved: &str, live: &str) -> TitleMatch {
    if saved.is_empty() || live.is_empty() {
        return TitleMatch::None;
    }
    if saved == live {
        return TitleMatch::Exact;
    }
    let live_parts = title_parts(live);
    let shared = title_parts(saved)
        .iter()
        .filter(|part| live_parts.contains(part))
        .count();
    match shared {
        0 => TitleMatch::None,
        n => TitleMatch::Parts(n.min(u8::MAX as usize) as u8),
    }
}

fn title_parts(title: &str) -> Vec<&str> {
    let mut parts = vec![title];
    for sep in TITLE_SEPARATORS {
        parts = parts.iter().flat_map(|p| p.split(sep)).collect();
    }
    parts
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect()
}

impl fmt::Display for Basis {
    /// "class, same workspace, title" — what decided a pairing, for the
    /// verbose report and the log. Never the title itself.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = vec![match self.by {
            MatchedBy::Class => "class",
            MatchedBy::InitialClass => "initial class",
        }
        .to_string()];
        if self.same_workspace {
            parts.push("same workspace".to_string());
        }
        match self.title {
            TitleMatch::None => {}
            TitleMatch::Parts(_) => parts.push("similar title".to_string()),
            TitleMatch::Exact => parts.push("same title".to_string()),
        }
        if self.tied > 0 {
            parts.push(format!(
                "a guess: {} other candidate{} alike, saved order chose",
                self.tied,
                if self.tied == 1 { "" } else { "s" }
            ));
        }
        f.write_str(&parts.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::LaunchInfo;
    use crate::workspace::WorkspaceRef;

    fn saved(class: &str, ws: i32, title: &str) -> SessionClient {
        SessionClient {
            class: class.to_string(),
            initial_class: class.to_string(),
            title: title.to_string(),
            workspace: WorkspaceRef::new(ws, ws.to_string()),
            monitor: "eDP-1".to_string(),
            at: [0, 0],
            size: [800, 600],
            floating: false,
            pinned: false,
            fullscreen: 0,
            focus_history_id: -1,
            group: None,
            group_position: None,
            identity: Default::default(),
            launch: LaunchInfo {
                argv: vec![class.to_string()],
                spawn: true,
                provider: None,
                provider_state: None,
            },
        }
    }

    fn live(class: &str, initial: &str, ws: i32, title: &str) -> HyprClient {
        HyprClient {
            address: format!("0x{class}{ws}"),
            class: class.to_string(),
            initial_class: initial.to_string(),
            title: title.to_string(),
            initial_title: String::new(),
            workspace: WorkspaceRef::new(ws, ws.to_string()),
            monitor: 0,
            at: [0, 0],
            size: [800, 600],
            floating: false,
            pinned: false,
            fullscreen: 0,
            focus_history_id: 0,
            pid: 1,
            mapped: true,
            grouped: vec![],
        }
    }

    fn pick(pending: &[SessionClient], window: &HyprClient) -> Option<(usize, Basis)> {
        let refs: Vec<&SessionClient> = pending.iter().collect();
        best(&refs, window)
    }

    /// Table: saved entries, one live window, the entry it must pair with.
    #[test]
    fn ranking_table() {
        let vscode = |ws, folder: &str| {
            saved(
                "code",
                ws,
                &format!("main.rs - {folder} - Visual Studio Code"),
            )
        };
        let cases: Vec<(&str, Vec<SessionClient>, HyprClient, Option<usize>)> = vec![
            (
                "three identical terminals: the one already on this workspace",
                vec![
                    saved("foot", 1, "foot"),
                    saved("foot", 2, "foot"),
                    saved("foot", 3, "foot"),
                ],
                live("foot", "foot", 3, "foot"),
                Some(2),
            ),
            (
                "two VS Code windows of one process: the title tells them apart",
                vec![vscode(1, "hyprwake"), vscode(2, "agent-stack")],
                live(
                    "code",
                    "code",
                    7,
                    "lib.rs - agent-stack - Visual Studio Code",
                ),
                Some(1),
            ),
            (
                "workspace agreement outranks a matching title",
                vec![vscode(1, "hyprwake"), vscode(2, "agent-stack")],
                live(
                    "code",
                    "code",
                    1,
                    "lib.rs - agent-stack - Visual Studio Code",
                ),
                Some(0),
            ),
            (
                "an exact title outranks a partial one",
                vec![
                    saved("google-chrome", 1, "Inbox - Work - Google Chrome"),
                    saved("google-chrome", 2, "Docs - Work - Google Chrome"),
                ],
                live(
                    "google-chrome",
                    "google-chrome",
                    5,
                    "Docs - Work - Google Chrome",
                ),
                Some(1),
            ),
            (
                "a title not set yet changes nothing: saved order",
                vec![vscode(1, "hyprwake"), vscode(2, "agent-stack")],
                live("code", "code", 7, ""),
                Some(0),
            ),
            (
                "a class that renamed itself after mapping is found by its initial class",
                vec![saved("org.gnome.Nautilus", 4, "")],
                live("nautilus", "org.gnome.Nautilus", 4, ""),
                Some(0),
            ),
            (
                "XWayland: the saved initial class is the live class",
                vec![{
                    let mut s = saved("Steam", 6, "");
                    s.initial_class = "steam".to_string();
                    s
                }],
                live("steam", "steam", 6, ""),
                Some(0),
            ),
            (
                "an exact class beats an initial-class match even off-workspace",
                vec![
                    {
                        let mut s = saved("other", 1, "");
                        s.initial_class = "foot".to_string();
                        s
                    },
                    saved("foot", 2, ""),
                ],
                live("foot", "foot", 1, ""),
                Some(1),
            ),
            (
                "no class in common, no match, whatever the title says",
                vec![saved("kitty", 1, "nvim")],
                live("foot", "foot", 1, "nvim"),
                None,
            ),
        ];
        for (name, pending, window, want) in cases {
            let got = pick(&pending, &window).map(|(i, _)| i);
            assert_eq!(got, want, "{name}");
        }
    }

    #[test]
    fn a_choice_by_saved_order_alone_is_reported_as_a_guess() {
        let pending = vec![
            saved("foot", 1, ""),
            saved("foot", 2, ""),
            saved("foot", 3, ""),
        ];
        let (i, basis) = pick(&pending, &live("foot", "foot", 9, "")).unwrap();
        assert_eq!(i, 0);
        assert_eq!(basis.tied, 2);
        assert!(basis.to_string().contains("a guess"), "{basis}");

        let (_, sure) = pick(&pending, &live("foot", "foot", 2, "")).unwrap();
        assert_eq!(sure.tied, 0);
        assert_eq!(sure.to_string(), "class, same workspace");
    }

    #[test]
    fn the_basis_never_repeats_the_title() {
        let pending = vec![saved("code", 1, "secret-project - Visual Studio Code")];
        let (_, basis) = pick(
            &pending,
            &live("code", "code", 1, "secret-project - Visual Studio Code"),
        )
        .unwrap();
        let text = basis.to_string();
        assert_eq!(text, "class, same workspace, same title");
        assert!(!text.contains("secret"));
    }

    #[test]
    fn pairing_a_desktop_makes_the_surest_pairs_first() {
        // The stray terminal is listed first, but the one already on
        // workspace 1 is the one saved there.
        let entries = [saved("foot", 1, ""), saved("foot", 2, "")];
        let saved_refs: Vec<&SessionClient> = entries.iter().collect();
        let windows = [live("foot", "foot", 9, ""), live("foot", "foot", 1, "")];
        let live_refs: Vec<&HyprClient> = windows.iter().collect();
        let pairs: Vec<Option<usize>> = pair_all(&saved_refs, &live_refs)
            .into_iter()
            .map(|p| p.map(|(i, _)| i))
            .collect();
        assert_eq!(pairs, vec![Some(1), Some(0)]);
    }

    #[test]
    fn pairing_all_uses_each_entry_once() {
        let entries = [
            saved("foot", 1, ""),
            saved("foot", 2, ""),
            saved("code", 3, ""),
        ];
        let saved_refs: Vec<&SessionClient> = entries.iter().collect();
        let windows = [
            live("foot", "foot", 2, ""),
            live("foot", "foot", 2, ""),
            live("foot", "foot", 2, ""),
            live("mpv", "mpv", 1, ""),
        ];
        let live_refs: Vec<&HyprClient> = windows.iter().collect();
        let pairs: Vec<Option<usize>> = pair_all(&saved_refs, &live_refs)
            .into_iter()
            .map(|p| p.map(|(i, _)| i))
            .collect();
        assert_eq!(pairs, vec![Some(1), Some(0), None, None]);
    }

    #[test]
    fn title_parts_split_on_the_usual_separators() {
        assert_eq!(
            title_parts("a.rs - hyprwake — Code | x · y"),
            vec!["a.rs", "hyprwake", "Code", "x", "y"]
        );
        assert_eq!(title_match("a - b", "c - b"), TitleMatch::Parts(1));
        assert_eq!(title_match("a - b", "a - b"), TitleMatch::Exact);
        assert_eq!(title_match("", "a"), TitleMatch::None);
    }
}
