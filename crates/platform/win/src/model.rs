//! The platform-neutral half of the agent: what each wheel shows for the dragged files,
//! where the overlay goes, how a pointer maps to a zone, and what a commit means.
//!
//! Nothing here touches the OS, so it is unit-tested on Linux and shared with the future
//! Linux overlay. The Win32 layer (`agent.rs`) only moves events and pixels.
//!
//! The agent never opens user files (PROJECT_GUIDE.md rule 3): menus come from file *names*.
//! Content sniffing happens when the job runs.

use std::path::{Path, PathBuf};

use swirvo_core::{FormatId, Preset};
use swirvo_jobs::tools::{Place, Tool, tools_for};
use swirvo_jobs::{Toolkit, wheel_items};
use swirvo_render::{CANVAS_H, CANVAS_W, CENTER, Scene, Sub, Theme, Wedge};
use swirvo_wheel::Frecency;
use swirvo_wheel::fsm::{State, WedgeShape, Wheel};
use swirvo_wheel::geom::{self, Zone};

/// The Convert sub-ring (docs/01 §6). Index = `Effect::Commit::preset`.
pub const PRESETS: [(&str, Preset); 3] = [
    ("Best", Preset::Best),
    ("Balanced", Preset::Balanced),
    ("Smallest", Preset::Smallest),
];

/// The Copy path sub-ring (docs/01 §9).
pub const PATH_STYLES: [(&str, PathStyle); 3] = [
    ("Quoted", PathStyle::Quoted),
    ("Forward /", PathStyle::Forward),
    ("WSL", PathStyle::Wsl),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    Plain,
    Quoted,
    Forward,
    Wsl,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Convert(FormatId),
    /// A tool or action from `swirvo_jobs::tools` (resize, merge PDFs, extract, zip…).
    Tool(Tool),
    ShowInFolder,
    CopyPath,
    OpenTerminal,
    /// "More…": every other target; dropping here opens the files in the Board.
    More,
    /// "More…" on the Tools wheel: the Board's full tool list for these files.
    MoreTools,
    /// Open the dragged files in the Board (batch convert, every target).
    OpenBoard,
}

impl Action {
    pub fn committable(&self) -> bool {
        true
    }
    fn sub_items(&self) -> &'static [&'static str] {
        match self {
            Action::Convert(_) => &["Best", "Balanced", "Smallest"],
            Action::CopyPath => &["Quoted", "Forward /", "WSL"],
            _ => &[],
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub label: String,
    pub caption: String,
    pub action: Action,
}

impl Entry {
    fn new(label: &str, caption: &str, action: Action) -> Entry {
        Entry {
            label: label.into(),
            caption: caption.into(),
            action,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Menu {
    pub wheel: Wheel,
    pub entries: Vec<Entry>,
    pub title: String,
    pub subtitle: String,
    /// The Terminal wedge's sub-ring: shells the host found, best first.
    pub shells: Vec<String>,
}

pub fn wheel_index(w: Wheel) -> usize {
    match w {
        Wheel::Convert => 0,
        Wheel::Tools => 1,
        Wheel::Actions => 2,
    }
}

fn tool_entry(t: Tool) -> Entry {
    let (label, caption) = t.label();
    Entry::new(label, caption, Action::Tool(t))
}

fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(2)).collect::<String>()
        )
    } else {
        s.to_string()
    }
}

impl Menu {
    pub fn build(
        tk: &Toolkit,
        wheel: Wheel,
        files: &[PathBuf],
        frecency: &Frecency,
        now: u64,
    ) -> Menu {
        let reg = tk.registry;
        let formats: Vec<Option<FormatId>> = files.iter().map(|f| reg.from_path(f)).collect();
        let all_known = !files.is_empty() && formats.iter().all(Option::is_some);
        let mut distinct: Vec<FormatId> = formats.iter().flatten().copied().collect();
        distinct.sort_by_key(|f| f.index());
        distinct.dedup();

        let title = ellipsize(
            &if files.len() == 1 {
                file_name(&files[0])
            } else {
                format!("{} files", files.len())
            },
            14,
        );
        let subtitle = match (distinct.as_slice(), all_known) {
            ([one], true) => reg.primary_ext(*one).to_ascii_uppercase(),
            (_, true) => "mixed types".into(),
            _ => "unknown type".into(),
        };

        let mut entries = Vec::new();
        match wheel {
            Wheel::Convert if all_known => {
                let (items, more) = wheel_items(tk, &distinct, frecency, now);
                entries.extend(items.into_iter().map(|i| Entry {
                    label: i.label,
                    caption: i.caption,
                    action: Action::Convert(i.target),
                }));
                if more > 0 {
                    entries.push(Entry {
                        label: "More…".into(),
                        caption: format!("{more} more"),
                        action: Action::More,
                    });
                }
            }
            Wheel::Convert => {}
            Wheel::Tools => {
                let tools: Vec<Tool> = tools_for(tk, files)
                    .into_iter()
                    .filter(|t| t.place() == Place::Tools)
                    .collect();
                if tools.len() > geom::MAX_WEDGES {
                    let shown = geom::MAX_WEDGES - 1;
                    entries.extend(tools[..shown].iter().copied().map(tool_entry));
                    entries.push(Entry {
                        label: "More…".into(),
                        caption: format!("{} more", tools.len() - shown),
                        action: Action::MoreTools,
                    });
                } else {
                    entries.extend(tools.into_iter().map(tool_entry));
                }
            }
            Wheel::Actions => {
                entries.push(Entry::new("Show", "In folder", Action::ShowInFolder));
                entries.push(Entry::new("Copy path", "Clipboard", Action::CopyPath));
                entries.push(Entry::new("Terminal", "Open here", Action::OpenTerminal));
                entries.push(Entry::new("Board", "Batch convert", Action::OpenBoard));
                let tools = tools_for(tk, files);
                entries.extend(
                    tools
                        .into_iter()
                        .filter(|t| t.place() == Place::Actions)
                        .map(tool_entry),
                );
            }
        }
        entries.truncate(geom::MAX_WEDGES);
        Menu {
            wheel,
            entries,
            title,
            subtitle,
            shells: Vec::new(),
        }
    }

    /// Offer these shells (labels from the host, best first) on the Terminal wedge.
    /// Dropping on the wedge opens the first one, so its caption names it; resting on
    /// the wedge shows the others. With one shell or none there is no sub-ring.
    pub fn with_shells(mut self, shells: &[String]) -> Menu {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.action == Action::OpenTerminal)
        else {
            return self;
        };
        if let Some(first) = shells.first()
            && *first != entry.label
        {
            entry.caption = first.clone();
        }
        if shells.len() > 1 {
            self.shells = shells.to_vec();
        }
        self
    }

    /// The sub-ring under an entry: presets, path styles, or this PC's shells.
    fn sub_items(&self, a: &Action) -> Vec<&str> {
        match a {
            Action::OpenTerminal => self.shells.iter().map(String::as_str).collect(),
            other => other.sub_items().to_vec(),
        }
    }

    pub fn layout(&self) -> Vec<WedgeShape> {
        self.entries
            .iter()
            .map(|e| WedgeShape {
                sub_items: self.sub_items(&e.action).len(),
                committable: e.action.committable(),
            })
            .collect()
    }

    /// Scene for the current machine state. `hub_cancel`: pointer is back on the hub.
    /// Scene for the current machine state. `hub_cancel`: pointer is back on the hub.
    pub fn scene(&self, state: &State, hub_cancel: bool, theme: Theme, scale: f32) -> Scene {
        let mut scene = Scene {
            wedges: self
                .entries
                .iter()
                .map(|e| Wedge {
                    label: e.label.clone(),
                    caption: e.caption.clone(),
                })
                .collect(),
            wheel: wheel_index(self.wheel),
            theme,
            scale,
            ..Scene::default()
        };
        self.refresh(&mut scene, state, hub_cancel);
        scene
    }

    /// Update a scene from [`Menu::scene`] in place for a new machine state. Hover changes
    /// don't allocate; hub text and sub-ring labels are rebuilt only when they change.
    pub fn refresh(&self, scene: &mut Scene, state: &State, hub_cancel: bool) {
        let open = match state {
            State::Open(o) => Some(o),
            _ => None,
        };
        scene.hover = open.and_then(|o| o.hover);
        match open.and_then(|o| o.sub) {
            Some(ring) => {
                if scene.sub.as_ref().is_none_or(|x| x.parent != ring.parent) {
                    let items = self
                        .entries
                        .get(ring.parent)
                        .map_or_else(Vec::new, |e| self.sub_items(&e.action));
                    scene.sub = Some(Sub {
                        parent: ring.parent,
                        items: items.iter().map(|s| s.to_string()).collect(),
                        hover: None,
                    });
                }
                if let Some(x) = scene.sub.as_mut() {
                    x.hover = open.and_then(|o| o.sub_hover);
                }
            }
            None => scene.sub = None,
        }
        scene.hit_radius = if scene.sub.is_some() {
            geom::R2 + 12.0
        } else {
            geom::R + geom::PUSH_BAND
        };
        let (title, subtitle) = if self.entries.is_empty() {
            ("Nothing here", "for this file yet")
        } else if hub_cancel {
            ("Cancel", "release to cancel")
        } else {
            (self.title.as_str(), self.subtitle.as_str())
        };
        if scene.hub_title != title || scene.hub_subtitle != subtitle {
            scene.hub_title = title.to_string();
            scene.hub_subtitle = subtitle.to_string();
        }
    }
}

/// Where the overlay window sits on screen, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f32,
}

impl Placement {
    /// Center the hub on the cursor, clamped so the wheel stays inside `work` (x, y, w, h).
    pub fn around(cursor: (i32, i32), work: (i32, i32, i32, i32), scale: f32) -> Placement {
        let scale = scale.clamp(0.5, 4.0);
        let w = (work.0 as f32, work.1 as f32, work.2 as f32, work.3 as f32);
        let (cx, cy) = geom::clamp_center(cursor.0 as f32, cursor.1 as f32, w, scale);
        Placement {
            left: (cx - CENTER.0 * scale).round() as i32,
            top: (cy - CENTER.1 * scale).round() as i32,
            width: (CANVAS_W * scale).ceil() as i32,
            height: (CANVAS_H * scale).ceil() as i32,
            scale,
        }
    }

    pub fn hub(&self) -> (f32, f32) {
        (
            self.left as f32 + CENTER.0 * self.scale,
            self.top as f32 + CENTER.1 * self.scale,
        )
    }

    /// Zone under a screen point, with the machine's current hover and sub-ring.
    pub fn zone(&self, pt: (i32, i32), state: &State, wedges: usize) -> Zone {
        let (hx, hy) = self.hub();
        let (dx, dy) = (
            (pt.0 as f32 - hx) / self.scale,
            (pt.1 as f32 - hy) / self.scale,
        );
        let (hover, sub) = match state {
            State::Open(o) => (o.hover, o.sub),
            _ => (None, None),
        };
        geom::hit(dx, dy, wedges, hover, sub.as_ref())
    }
}

/// What a commit asks the host to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Work {
    Convert {
        files: Vec<PathBuf>,
        to: FormatId,
        preset: Preset,
    },
    Tool {
        tool: Tool,
        files: Vec<PathBuf>,
    },
    ShowInFolder {
        file: PathBuf,
    },
    CopyPath {
        text: String,
    },
    /// Open shell `shell` (index into the host's list; 0 = default) in the folder of
    /// `at`, or in `at` itself when it is a folder. The host checks which, off the UI
    /// thread (it is a file-system call).
    OpenTerminal {
        at: PathBuf,
        shell: usize,
    },
    /// Open the Board with these files; `tool` shows the Tools tab ("" the list, or a
    /// tool id with its options).
    OpenBoard {
        files: Vec<PathBuf>,
        tool: Option<String>,
    },
    Nothing,
}

pub fn resolve(menu: &Menu, wedge: usize, sub: Option<usize>, files: &[PathBuf]) -> Work {
    let Some(entry) = menu.entries.get(wedge) else {
        return Work::Nothing;
    };
    match entry.action {
        Action::Convert(to) => {
            let preset = sub
                .and_then(|j| PRESETS.get(j))
                .map_or(Preset::Balanced, |p| p.1);
            Work::Convert {
                files: files.to_vec(),
                to,
                preset,
            }
        }
        // Tools that need options (a password, watermark text) open the Board's form.
        Action::Tool(tool) if tool.asks() => Work::OpenBoard {
            files: files.to_vec(),
            tool: Some(tool.id().to_string()),
        },
        Action::Tool(tool) => Work::Tool {
            tool,
            files: files.to_vec(),
        },
        Action::ShowInFolder => files
            .first()
            .map_or(Work::Nothing, |f| Work::ShowInFolder { file: f.clone() }),
        Action::CopyPath => {
            let style = sub
                .and_then(|j| PATH_STYLES.get(j))
                .map_or(PathStyle::Plain, |p| p.1);
            Work::CopyPath {
                text: format_paths(files, style),
            }
        }
        Action::OpenTerminal => files.first().map_or(Work::Nothing, |f| Work::OpenTerminal {
            at: f.clone(),
            shell: sub.unwrap_or(0),
        }),
        Action::More | Action::OpenBoard => Work::OpenBoard {
            files: files.to_vec(),
            tool: None,
        },
        Action::MoreTools => Work::OpenBoard {
            files: files.to_vec(),
            tool: Some(String::new()),
        },
    }
}

/// One path per line (CRLF, so it pastes cleanly into Windows apps).
pub fn format_paths(files: &[PathBuf], style: PathStyle) -> String {
    let one = |p: &PathBuf| {
        let s = p.to_string_lossy();
        match style {
            PathStyle::Plain => s.into_owned(),
            PathStyle::Quoted => format!("\"{s}\""),
            PathStyle::Forward => s.replace('\\', "/"),
            PathStyle::Wsl => wsl_path(&s),
        }
    };
    files.iter().map(one).collect::<Vec<_>>().join("\r\n")
}

/// `C:\Users\me\a.txt` → `/mnt/c/Users/me/a.txt`; `\\?\C:\…` too. Anything else (UNC,
/// already-POSIX) only gets its separators flipped.
fn wsl_path(s: &str) -> String {
    let s = s.strip_prefix(r"\\?\").unwrap_or(s);
    let b = s.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        let rest = s[2..].replace('\\', "/");
        let rest = rest.trim_start_matches('/');
        format!("/mnt/{}/{}", (b[0] as char).to_ascii_lowercase(), rest)
            .trim_end_matches('/')
            .to_string()
    } else {
        s.replace('\\', "/")
    }
}

/// tiny-skia's premultiplied RGBA → the premultiplied BGRA a DIB/`UpdateLayeredWindow` wants.
pub fn rgba_to_bgra(src: &[u8], dst: &mut [u8]) {
    for (s, d) in src
        .as_chunks::<4>()
        .0
        .iter()
        .zip(dst.as_chunks_mut::<4>().0)
    {
        *d = [s[2], s[1], s[0], s[3]];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swirvo_wheel::fsm::{Effect, Event, Machine};

    fn tk() -> Toolkit {
        Toolkit::with_builtin_engines().unwrap()
    }

    #[test]
    fn convert_menu_from_names_only() {
        let tk = tk();
        // The file doesn't exist: menus must not need to read it.
        let files = vec![PathBuf::from("/nowhere/IMG_2041.png")];
        let m = Menu::build(&tk, Wheel::Convert, &files, &Frecency::default(), 0);
        assert!(!m.entries.is_empty() && m.entries.len() <= geom::MAX_WEDGES);
        assert_eq!(m.entries[0].label, "JPG", "prior: images go to JPG first");
        assert!(
            m.entries.iter().all(|e| e.label != "PNG"),
            "never offer the source format"
        );
        assert_eq!(
            (m.title.as_str(), m.subtitle.as_str()),
            ("IMG_2041.png", "PNG")
        );
        let lay = m.layout();
        assert!(lay.iter().any(|w| w.sub_items == 3 && w.committable));
        if m.entries.last().is_some_and(|e| e.action == Action::More) {
            assert!(lay.last().unwrap().committable, "More… opens the Board");
            assert_eq!(
                resolve(
                    &m,
                    m.entries.len() - 1,
                    None,
                    &[PathBuf::from("IMG_2041.png")]
                ),
                Work::OpenBoard {
                    files: vec![PathBuf::from("IMG_2041.png")],
                    tool: None,
                }
            );
        }
    }

    #[test]
    fn unknown_and_mixed() {
        let tk = tk();
        let m = Menu::build(
            &tk,
            Wheel::Convert,
            &[PathBuf::from("a.unknownext")],
            &Frecency::default(),
            0,
        );
        assert!(m.entries.is_empty());
        assert_eq!(
            m.scene(&State::Idle, false, Theme::Light, 1.0).hub_title,
            "Nothing here"
        );
        let m = Menu::build(
            &tk,
            Wheel::Convert,
            &[PathBuf::from("a.png"), PathBuf::from("b.jpg")],
            &Frecency::default(),
            0,
        );
        assert_eq!(
            (m.title.as_str(), m.subtitle.as_str()),
            ("2 files", "mixed types")
        );
        assert!(
            m.entries.iter().any(|e| e.label == "WEBP"),
            "common targets of PNG and JPG"
        );
    }

    #[test]
    fn pdf_tools_overflow_into_the_board() {
        let tk = tk();
        let pdf = [PathBuf::from("Report.pdf")];
        let t = Menu::build(&tk, Wheel::Tools, &pdf, &Frecency::default(), 0);
        assert_eq!(t.entries.len(), geom::MAX_WEDGES);
        assert_eq!(t.entries[0].action, Action::Tool(Tool::PdfCompress));
        let last = t.entries.len() - 1;
        assert_eq!(t.entries[last].action, Action::MoreTools);
        assert_eq!(
            resolve(&t, last, None, &pdf),
            Work::OpenBoard {
                files: pdf.to_vec(),
                tool: Some(String::new())
            }
        );
        // Protect needs a password first: it opens the Board's form instead of running.
        let i = t
            .entries
            .iter()
            .position(|e| e.action == Action::Tool(Tool::PdfProtect))
            .expect("Protect is on the wheel");
        assert_eq!(
            resolve(&t, i, None, &pdf),
            Work::OpenBoard {
                files: pdf.to_vec(),
                tool: Some("pdf-protect".into())
            }
        );
        // Compress runs straight away with its defaults.
        assert!(matches!(resolve(&t, 0, None, &pdf), Work::Tool { .. }));
    }

    #[test]
    fn tools_and_actions() {
        let tk = tk();
        let t = Menu::build(
            &tk,
            Wheel::Tools,
            &[PathBuf::from("Assets.tar.gz")],
            &Frecency::default(),
            0,
        );
        assert_eq!(t.entries[0].action, Action::Tool(Tool::ExtractHere));
        assert!(t.entries.iter().all(|e| e.caption != "Coming soon"));
        let a = Menu::build(
            &tk,
            Wheel::Actions,
            &[PathBuf::from("x.bin")],
            &Frecency::default(),
            0,
        );
        assert!(a.layout().iter().filter(|w| w.committable).count() >= 3);
    }

    #[test]
    fn placement_round_trip_and_clamp() {
        let work = (0, 0, 1920, 1040);
        let p = Placement::around((960, 520), work, 1.5);
        assert_eq!(p.hub(), (960.0, 520.0));
        assert_eq!(p.width, (CANVAS_W * 1.5).ceil() as i32);
        // Near the screen corner the wheel moves inside the work area.
        let c = Placement::around((5, 5), work, 1.0);
        let (hx, hy) = c.hub();
        assert!(hx >= geom::R && hy >= geom::R);
        assert_eq!(c.zone((hx as i32, hy as i32), &State::Idle, 8), Zone::Hub);
        // Straight up from the hub, mid-band, is wedge 0.
        let mut m = Machine::new();
        m.set_layout(vec![
            WedgeShape {
                sub_items: 0,
                committable: true
            };
            8
        ]);
        m.step(Event::Trigger {
            wheel: Wheel::Convert,
            button_held: true,
            now: 0,
        });
        m.step(Event::DragEnter { files: 1, now: 1 });
        assert_eq!(
            c.zone((hx as i32, (hy - 100.0) as i32), m.state(), 8),
            Zone::Ring(0)
        );
    }

    #[test]
    fn commit_maps_to_work() {
        let tk = tk();
        let files = vec![PathBuf::from(r"C:\Users\me\My Photo.png")];
        let m = Menu::build(&tk, Wheel::Convert, &files, &Frecency::default(), 0);
        let Work::Convert { to, preset, .. } = resolve(&m, 0, Some(2), &files) else {
            panic!()
        };
        assert_eq!((tk.registry.key(to), preset), ("jpg", Preset::Smallest));
        let a = Menu::build(&tk, Wheel::Actions, &files, &Frecency::default(), 0);
        let copy = a
            .entries
            .iter()
            .position(|e| e.action == Action::CopyPath)
            .unwrap();
        assert_eq!(
            resolve(&a, copy, Some(2), &files),
            Work::CopyPath {
                text: "/mnt/c/Users/me/My Photo.png".into()
            }
        );
        assert_eq!(
            resolve(&a, copy, None, &files),
            Work::CopyPath {
                text: r"C:\Users\me\My Photo.png".into()
            }
        );
        assert_eq!(resolve(&a, 99, None, &files), Work::Nothing);
    }

    #[test]
    fn terminal_offers_the_shells_found() {
        let tk = tk();
        let files = vec![PathBuf::from(r"C:\work\notes.txt")];
        let shells: Vec<String> = ["PowerShell", "Cmd", "Ubuntu", "Git Bash"]
            .map(String::from)
            .to_vec();
        let a =
            Menu::build(&tk, Wheel::Actions, &files, &Frecency::default(), 0).with_shells(&shells);
        let t = a
            .entries
            .iter()
            .position(|e| e.action == Action::OpenTerminal)
            .unwrap();
        assert_eq!(a.entries[t].caption, "PowerShell", "the default is named");
        assert_eq!(a.layout()[t].sub_items, 4);
        let at = files[0].clone();
        assert_eq!(
            resolve(&a, t, Some(2), &files),
            Work::OpenTerminal {
                at: at.clone(),
                shell: 2
            }
        );
        assert_eq!(
            resolve(&a, t, None, &files),
            Work::OpenTerminal { at, shell: 0 }
        );
        // One shell: no sub-ring. Windows Terminal first: the caption stays "Open here".
        let one = Menu::build(&tk, Wheel::Actions, &files, &Frecency::default(), 0)
            .with_shells(&["Terminal".to_string()]);
        assert_eq!(one.layout()[t].sub_items, 0);
        assert_eq!(one.entries[t].caption, "Open here");
        // Other wheels are untouched.
        let c =
            Menu::build(&tk, Wheel::Convert, &files, &Frecency::default(), 0).with_shells(&shells);
        assert!(c.shells.is_empty());
    }

    #[test]
    fn full_gesture_drives_a_commit() {
        let tk = tk();
        let files = vec![PathBuf::from("shot.png")];
        let menu = Menu::build(&tk, Wheel::Convert, &files, &Frecency::default(), 0);
        let place = Placement::around((500, 500), (0, 0, 1920, 1080), 1.0);
        let mut m = Machine::new();
        m.step(Event::Trigger {
            wheel: Wheel::Convert,
            button_held: true,
            now: 0,
        });
        let fx = m.step(Event::DragEnter { files: 1, now: 20 });
        assert!(fx.contains(&Effect::ShowWheel(Wheel::Convert)));
        m.set_layout(menu.layout());
        let (hx, hy) = place.hub();
        let up = (hx as i32, (hy - 100.0) as i32);
        let z = place.zone(up, m.state(), menu.entries.len());
        m.step(Event::Pointer { zone: z, now: 40 });
        let scene = menu.scene(m.state(), false, Theme::Dark, 1.0);
        assert_eq!(scene.hover, Some(0));
        let fx = m.step(Event::Drop { zone: z, now: 60 });
        let commit = fx.iter().find_map(|f| match f {
            Effect::Commit { wedge, preset, .. } => Some((*wedge, *preset)),
            _ => None,
        });
        assert_eq!(commit, Some((0, None)));
        assert!(matches!(
            resolve(&menu, 0, None, &files),
            Work::Convert {
                preset: Preset::Balanced,
                ..
            }
        ));
    }

    #[test]
    fn paths_and_pixels() {
        assert_eq!(wsl_path(r"\\?\D:\x\y"), "/mnt/d/x/y");
        assert_eq!(wsl_path(r"\\server\share\f"), "//server/share/f");
        assert_eq!(wsl_path(r"C:\"), "/mnt/c");
        let files = vec![PathBuf::from("a b"), PathBuf::from("c")];
        assert_eq!(format_paths(&files, PathStyle::Quoted), "\"a b\"\r\n\"c\"");
        let mut out = [0u8; 8];
        rgba_to_bgra(&[1, 2, 3, 4, 5, 6, 7, 8], &mut out);
        assert_eq!(out, [3, 2, 1, 4, 7, 6, 5, 8]);
    }
}
