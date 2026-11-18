//! Gesture state machine (docs/02 §3.9, docs/04 §5).
//!
//! `Machine::step(event) -> Vec<Effect>`. Platform layers translate OS input into
//! [`Event`]s and perform the [`Effect`]s; timers arrive as [`Event::Tick`].
//!
//! Safety by construction: [`DropEffect`] has no `Move` variant, so no code path can tell
//! the drag source to delete the file.

use crate::geom::{EXIT_GRACE, SubRing, Zone};
use crate::rank::Millis;

pub const PROBE_TIMEOUT_MS: Millis = 150;
pub const DWELL_MS: Millis = 450;
pub const FADE_AFTER_MS: Millis = 300;
pub const RECEIPT_MS: Millis = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wheel {
    Convert,
    Tools,
    Actions,
}

impl Wheel {
    pub fn next(self, forward: bool) -> Wheel {
        use Wheel::*;
        match (self, forward) {
            (Convert, true) | (Actions, false) => Tools,
            (Tools, true) | (Convert, false) => Actions,
            (Actions, true) | (Tools, false) => Convert,
        }
    }
}

/// What the machine needs to know about each wedge of the current wheel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WedgeShape {
    /// Number of sub-ring items (0 = none).
    pub sub_items: usize,
    /// False for "More…" (opens a ring, never runs a job).
    pub committable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Next,
    Prev,
    Up,
    Down,
    Enter,
    Tab { back: bool },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// A trigger modifier went down. `button_held`: primary/secondary mouse button down.
    Trigger {
        wheel: Wheel,
        button_held: bool,
        now: Millis,
    },
    /// OLE/XDND/Wayland drag entered the probe or wheel.
    DragEnter {
        files: usize,
        now: Millis,
    },
    /// The drag left our window (no more position updates will arrive).
    DragLeave {
        now: Millis,
    },
    Pointer {
        zone: Zone,
        now: Millis,
    },
    Drop {
        zone: Zone,
        now: Millis,
    },
    /// Mouse button released without a drop on us (dropped elsewhere or cancelled).
    ButtonUp {
        now: Millis,
    },
    Escape,
    /// Global shortcut with a selection: opens keyboard mode.
    Shortcut {
        files: usize,
        now: Millis,
    },
    Key(Key),
    /// The scheduler accepted the committed job.
    JobAccepted {
        now: Millis,
    },
    Tick {
        now: Millis,
    },
}

/// Never `Move`: see module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropEffect {
    Copy,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    ShowProbe,
    HideProbe,
    /// Host computes the items for this wheel and calls [`Machine::set_layout`].
    ShowWheel(Wheel),
    SwitchWheel(Wheel),
    HideWheel,
    Highlight(Option<usize>),
    /// The hub is armed as "Cancel" (only after the pointer left it once).
    HubCancel(bool),
    OpenSub(usize),
    CloseSub,
    HighlightSub(Option<usize>),
    Fade(bool),
    /// Reply to the drag source for this drop. Exactly one per `Drop` event.
    DropResult(DropEffect),
    Commit {
        wheel: Wheel,
        wedge: usize,
        preset: Option<usize>,
    },
    ShowReceipt,
    HideReceipt,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Open {
    pub wheel: Wheel,
    pub keyboard: bool,
    pub hover: Option<usize>,
    pub hover_since: Millis,
    pub sub: Option<SubRing>,
    pub sub_hover: Option<usize>,
    pub outside_since: Option<Millis>,
    pub faded: bool,
    pub left_hub: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State {
    Idle,
    Probing {
        wheel: Wheel,
        since: Millis,
    },
    Open(Open),
    /// A job was committed; waiting for the scheduler to accept it.
    Committing,
    Receipt {
        since: Millis,
    },
}

#[derive(Debug, Clone)]
pub struct Machine {
    state: State,
    layout: Vec<WedgeShape>,
}

impl Default for Machine {
    fn default() -> Self {
        Machine {
            state: State::Idle,
            layout: Vec::new(),
        }
    }
}

impl Machine {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn state(&self) -> &State {
        &self.state
    }
    pub fn layout(&self) -> &[WedgeShape] {
        &self.layout
    }
    /// Set the current wheel's wedges (after `ShowWheel`/`SwitchWheel`, or a filter).
    pub fn set_layout(&mut self, layout: Vec<WedgeShape>) {
        self.layout = layout;
        if let State::Open(o) = &mut self.state {
            if o.hover.is_some_and(|h| h >= self.layout.len()) {
                o.hover = if self.layout.is_empty() {
                    None
                } else {
                    Some(0)
                };
            }
            o.sub = None;
            o.sub_hover = None;
        }
    }

    fn open(wheel: Wheel, keyboard: bool, now: Millis) -> Open {
        Open {
            wheel,
            keyboard,
            hover: keyboard.then_some(0),
            hover_since: now,
            sub: None,
            sub_hover: None,
            outside_since: None,
            faded: false,
            left_hub: false,
        }
    }

    pub fn step(&mut self, ev: Event) -> Vec<Effect> {
        use Effect as F;
        let mut fx = Vec::new();
        let layout = &self.layout;
        let next = match (self.state, ev) {
            // ---------------- escape always wins ----------------
            (State::Probing { .. }, Event::Escape) => {
                fx.push(F::HideProbe);
                State::Idle
            }
            (State::Open(_), Event::Escape) => {
                fx.push(F::HideWheel);
                State::Idle
            }
            (State::Receipt { .. }, Event::Escape) => {
                fx.push(F::HideReceipt);
                State::Idle
            }
            (s @ (State::Idle | State::Committing), Event::Escape) => s,

            // Every Drop gets exactly one DropResult, whatever the state.
            (State::Open(o), Event::Drop { zone, .. }) => {
                let target = if o.faded || o.keyboard {
                    None
                } else {
                    match zone {
                        Zone::Ring(i) if layout.get(i).is_some_and(|w| w.committable) => {
                            Some((i, None))
                        }
                        Zone::Sub(j) => o.sub.filter(|s| j < s.items).map(|s| (s.parent, Some(j))),
                        Zone::Near { .. } if o.sub.is_none() => o
                            .hover
                            .filter(|&h| layout.get(h).is_some_and(|w| w.committable))
                            .map(|h| (h, None)),
                        _ => None,
                    }
                };
                fx.push(F::HideWheel);
                match target {
                    Some((wedge, preset)) => {
                        fx.push(F::DropResult(DropEffect::Copy));
                        fx.push(F::Commit {
                            wheel: o.wheel,
                            wedge,
                            preset,
                        });
                        State::Committing
                    }
                    None => {
                        fx.push(F::DropResult(DropEffect::None));
                        State::Idle
                    }
                }
            }
            (State::Probing { .. }, Event::Drop { .. }) => {
                fx.push(F::HideProbe);
                fx.push(F::DropResult(DropEffect::None));
                State::Idle
            }
            (s, Event::Drop { .. }) => {
                fx.push(F::DropResult(DropEffect::None));
                s
            }

            // ---------------- idle / receipt ----------------
            (
                State::Idle | State::Receipt { .. },
                Event::Trigger {
                    wheel,
                    button_held: true,
                    now,
                },
            ) => {
                if matches!(self.state, State::Receipt { .. }) {
                    fx.push(F::HideReceipt);
                }
                fx.push(F::ShowProbe);
                State::Probing { wheel, since: now }
            }
            (State::Idle | State::Receipt { .. }, Event::Shortcut { files, now }) if files > 0 => {
                if matches!(self.state, State::Receipt { .. }) {
                    fx.push(F::HideReceipt);
                }
                fx.push(F::ShowWheel(Wheel::Convert));
                fx.push(F::Highlight(Some(0)));
                State::Open(Self::open(Wheel::Convert, true, now))
            }
            (State::Receipt { since }, Event::Tick { now })
                if now.saturating_sub(since) >= RECEIPT_MS =>
            {
                fx.push(F::HideReceipt);
                State::Idle
            }

            // ---------------- probing ----------------
            (State::Probing { wheel, .. }, Event::DragEnter { files, now }) => {
                if files > 0 {
                    fx.push(F::ShowWheel(wheel));
                    State::Open(Self::open(wheel, false, now))
                } else {
                    fx.push(F::HideProbe);
                    State::Idle
                }
            }
            (State::Probing { since, .. }, Event::Tick { now })
                if now.saturating_sub(since) >= PROBE_TIMEOUT_MS =>
            {
                fx.push(F::HideProbe);
                State::Idle
            }
            (State::Probing { .. }, Event::ButtonUp { .. }) => {
                fx.push(F::HideProbe);
                State::Idle
            }
            (State::Probing { since, .. }, Event::Trigger { wheel, .. }) => {
                State::Probing { wheel, since }
            }

            // ---------------- open ----------------
            (State::Open(mut o), Event::Trigger { wheel, .. }) if wheel != o.wheel => {
                Self::reset_for_switch(&mut o, wheel, &mut fx);
                State::Open(o)
            }
            (State::Open(mut o), Event::Key(Key::Tab { back })) => {
                let w = o.wheel.next(!back);
                Self::reset_for_switch(&mut o, w, &mut fx);
                if o.keyboard {
                    o.hover = Some(0);
                    fx.push(F::Highlight(Some(0)));
                }
                State::Open(o)
            }
            (State::Open(mut o), Event::Pointer { zone, now }) if !o.keyboard => {
                Self::on_pointer(&mut o, zone, now, layout, &mut fx);
                State::Open(o)
            }
            (State::Open(mut o), Event::DragLeave { now }) => {
                o.outside_since.get_or_insert(now);
                State::Open(o)
            }
            (State::Open(mut o), Event::Tick { now }) => {
                if !o.faded
                    && o.sub.is_none()
                    && let Some(h) = o.hover
                {
                    let n = layout.get(h).map_or(0, |w| w.sub_items);
                    if n > 0 && now.saturating_sub(o.hover_since) >= DWELL_MS {
                        o.sub = Some(SubRing {
                            parent: h,
                            parent_count: layout.len(),
                            items: n,
                        });
                        fx.push(F::OpenSub(h));
                    }
                }
                if let Some(t) = o.outside_since
                    && !o.faded
                    && now.saturating_sub(t) >= FADE_AFTER_MS
                {
                    o.faded = true;
                    if o.sub.take().is_some() {
                        fx.push(F::CloseSub);
                    }
                    o.hover = None;
                    fx.push(F::Highlight(None));
                    fx.push(F::Fade(true));
                }
                State::Open(o)
            }
            (State::Open(_), Event::ButtonUp { .. }) => {
                fx.push(F::HideWheel);
                State::Idle
            }
            (State::Open(mut o), Event::Key(k)) if o.keyboard => {
                match Self::on_key(&mut o, k, layout, &mut fx) {
                    Some(commit) => {
                        fx.push(F::HideWheel);
                        fx.push(commit);
                        State::Committing
                    }
                    None => State::Open(o),
                }
            }

            // ---------------- committing ----------------
            (State::Committing, Event::JobAccepted { now }) => {
                fx.push(F::ShowReceipt);
                State::Receipt { since: now }
            }

            (s, _) => s,
        };
        self.state = next;
        fx
    }

    fn reset_for_switch(o: &mut Open, wheel: Wheel, fx: &mut Vec<Effect>) {
        o.wheel = wheel;
        o.hover = None;
        o.sub = None;
        o.sub_hover = None;
        fx.push(Effect::SwitchWheel(wheel));
    }

    fn on_pointer(
        o: &mut Open,
        zone: Zone,
        now: Millis,
        layout: &[WedgeShape],
        fx: &mut Vec<Effect>,
    ) {
        use Effect as F;
        let far = matches!(zone, Zone::Outside { dist } if dist > EXIT_GRACE);
        if far {
            o.outside_since.get_or_insert(now);
        } else {
            o.outside_since = None;
            if o.faded {
                o.faded = false;
                fx.push(F::Fade(false));
            }
        }
        if o.faded {
            return;
        }
        let was_hub = !o.left_hub;
        if zone != Zone::Hub {
            o.left_hub = true;
        }
        let set_hover = |o: &mut Open, h: Option<usize>, fx: &mut Vec<Effect>| {
            if o.hover != h {
                o.hover = h;
                o.hover_since = now;
                fx.push(F::Highlight(h));
            }
        };
        match zone {
            Zone::Hub => {
                if o.sub.take().is_some() {
                    fx.push(F::CloseSub);
                }
                set_hover(o, None, fx);
                if o.left_hub {
                    fx.push(F::HubCancel(true));
                }
            }
            Zone::Gap | Zone::Outside { .. } => {
                if o.sub.is_none() {
                    set_hover(o, None, fx);
                } else if o.sub_hover.take().is_some() {
                    fx.push(F::HighlightSub(None));
                }
            }
            Zone::Ring(i) => {
                if o.sub.is_some_and(|s| s.parent != i) {
                    o.sub = None;
                    o.sub_hover = None;
                    fx.push(F::CloseSub);
                } else if o.sub_hover.take().is_some() {
                    fx.push(F::HighlightSub(None));
                }
                set_hover(o, Some(i).filter(|&i| i < layout.len()), fx);
            }
            Zone::Sub(j) => {
                if o.sub.is_some_and(|s| j < s.items) && o.sub_hover != Some(j) {
                    o.sub_hover = Some(j);
                    fx.push(F::HighlightSub(Some(j)));
                }
            }
            Zone::Near { wedge } => {
                // Pushing past the rim of the hovered wedge opens its sub-ring at once.
                if o.sub.is_none() && o.hover == Some(wedge) {
                    if let Some(w) = layout.get(wedge).filter(|w| w.sub_items > 0) {
                        o.sub = Some(SubRing {
                            parent: wedge,
                            parent_count: layout.len(),
                            items: w.sub_items,
                        });
                        fx.push(F::OpenSub(wedge));
                    }
                } else if o.sub.is_none() {
                    set_hover(o, Some(wedge).filter(|&i| i < layout.len()), fx);
                }
            }
        }
        if was_hub && o.left_hub {
            fx.push(F::HubCancel(false));
        }
    }

    fn on_key(o: &mut Open, k: Key, layout: &[WedgeShape], fx: &mut Vec<Effect>) -> Option<Effect> {
        use Effect as F;
        let n = layout.len();
        if n == 0 {
            return None;
        }
        match k {
            Key::Next | Key::Prev => {
                let fwd = k == Key::Next;
                if let Some(s) = o.sub {
                    let j = o.sub_hover.map_or(0, |j| {
                        if fwd {
                            (j + 1) % s.items
                        } else {
                            (j + s.items - 1) % s.items
                        }
                    });
                    o.sub_hover = Some(j);
                    fx.push(F::HighlightSub(Some(j)));
                } else {
                    let h = o
                        .hover
                        .map_or(0, |h| if fwd { (h + 1) % n } else { (h + n - 1) % n });
                    o.hover = Some(h);
                    fx.push(F::Highlight(Some(h)));
                }
                None
            }
            Key::Up => {
                if let Some(h) = o.hover.filter(|_| o.sub.is_none())
                    && let Some(w) = layout.get(h).filter(|w| w.sub_items > 0)
                {
                    o.sub = Some(SubRing {
                        parent: h,
                        parent_count: n,
                        items: w.sub_items,
                    });
                    o.sub_hover = Some(0);
                    fx.push(F::OpenSub(h));
                    fx.push(F::HighlightSub(Some(0)));
                }
                None
            }
            Key::Down => {
                if o.sub.take().is_some() {
                    o.sub_hover = None;
                    fx.push(F::CloseSub);
                }
                None
            }
            Key::Enter => {
                if let (Some(s), Some(j)) = (o.sub, o.sub_hover) {
                    return Some(F::Commit {
                        wheel: o.wheel,
                        wedge: s.parent,
                        preset: Some(j),
                    });
                }
                let h = o.hover?;
                if layout.get(h)?.committable {
                    Some(F::Commit {
                        wheel: o.wheel,
                        wedge: h,
                        preset: None,
                    })
                } else {
                    // "More…": Enter opens its ring instead.
                    let items = layout[h].sub_items;
                    if items > 0 && o.sub.is_none() {
                        o.sub = Some(SubRing {
                            parent: h,
                            parent_count: n,
                            items,
                        });
                        o.sub_hover = Some(0);
                        fx.push(F::OpenSub(h));
                        fx.push(F::HighlightSub(Some(0)));
                    }
                    None
                }
            }
            Key::Tab { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn layout() -> Vec<WedgeShape> {
        let mut l = vec![
            WedgeShape {
                sub_items: 4,
                committable: true
            };
            7
        ];
        l.push(WedgeShape {
            sub_items: 3,
            committable: false,
        }); // More…
        l
    }

    fn open_machine() -> Machine {
        let mut m = Machine::new();
        assert_eq!(
            m.step(Event::Trigger {
                wheel: Wheel::Convert,
                button_held: true,
                now: 0
            }),
            vec![Effect::ShowProbe]
        );
        assert_eq!(
            m.step(Event::DragEnter { files: 1, now: 5 }),
            vec![Effect::ShowWheel(Wheel::Convert)]
        );
        m.set_layout(layout());
        m
    }

    #[test]
    fn happy_path_drop_on_wedge() {
        let mut m = open_machine();
        m.step(Event::Pointer {
            zone: Zone::Ring(2),
            now: 20,
        });
        let fx = m.step(Event::Drop {
            zone: Zone::Ring(2),
            now: 30,
        });
        assert!(fx.contains(&Effect::DropResult(DropEffect::Copy)));
        assert!(fx.contains(&Effect::Commit {
            wheel: Wheel::Convert,
            wedge: 2,
            preset: None
        }));
        assert_eq!(*m.state(), State::Committing);
        m.step(Event::JobAccepted { now: 40 });
        m.step(Event::Tick {
            now: 40 + RECEIPT_MS,
        });
        assert_eq!(*m.state(), State::Idle);
    }

    #[test]
    fn dwell_opens_presets_and_drop_on_preset() {
        let mut m = open_machine();
        m.step(Event::Pointer {
            zone: Zone::Ring(1),
            now: 100,
        });
        assert!(
            m.step(Event::Tick {
                now: 100 + DWELL_MS - 1
            })
            .is_empty()
        );
        assert_eq!(
            m.step(Event::Tick {
                now: 100 + DWELL_MS
            }),
            vec![Effect::OpenSub(1)]
        );
        m.step(Event::Pointer {
            zone: Zone::Sub(2),
            now: 700,
        });
        let fx = m.step(Event::Drop {
            zone: Zone::Sub(2),
            now: 710,
        });
        assert!(fx.contains(&Effect::Commit {
            wheel: Wheel::Convert,
            wedge: 1,
            preset: Some(2)
        }));
    }

    #[test]
    fn hub_drop_cancels_and_more_is_not_committable() {
        let mut m = open_machine();
        let fx = m.step(Event::Drop {
            zone: Zone::Hub,
            now: 10,
        });
        assert_eq!(
            fx,
            vec![Effect::HideWheel, Effect::DropResult(DropEffect::None)]
        );
        let mut m = open_machine();
        let fx = m.step(Event::Drop {
            zone: Zone::Ring(7),
            now: 10,
        });
        assert!(fx.contains(&Effect::DropResult(DropEffect::None)));
    }

    #[test]
    fn probe_times_out_without_drag() {
        let mut m = Machine::new();
        m.step(Event::Trigger {
            wheel: Wheel::Tools,
            button_held: true,
            now: 0,
        });
        assert_eq!(
            m.step(Event::Tick {
                now: PROBE_TIMEOUT_MS
            }),
            vec![Effect::HideProbe]
        );
        assert_eq!(*m.state(), State::Idle);
        // Modifier without a held button never shows anything.
        assert!(
            m.step(Event::Trigger {
                wheel: Wheel::Tools,
                button_held: false,
                now: 1000
            })
            .is_empty()
        );
    }

    #[test]
    fn fades_after_leaving_and_recovers() {
        let mut m = open_machine();
        m.step(Event::Pointer {
            zone: Zone::Outside { dist: 100.0 },
            now: 50,
        });
        let fx = m.step(Event::Tick {
            now: 50 + FADE_AFTER_MS,
        });
        assert!(fx.contains(&Effect::Fade(true)));
        let fx = m.step(Event::Pointer {
            zone: Zone::Ring(0),
            now: 500,
        });
        assert!(fx.contains(&Effect::Fade(false)));
    }

    #[test]
    fn keyboard_mode() {
        let mut m = Machine::new();
        m.step(Event::Shortcut { files: 2, now: 0 });
        m.set_layout(layout());
        m.step(Event::Key(Key::Next));
        m.step(Event::Key(Key::Up));
        m.step(Event::Key(Key::Next));
        let fx = m.step(Event::Key(Key::Enter));
        assert!(fx.contains(&Effect::Commit {
            wheel: Wheel::Convert,
            wedge: 1,
            preset: Some(1)
        }));
    }

    fn zone() -> impl Strategy<Value = Zone> {
        prop_oneof![
            Just(Zone::Hub),
            Just(Zone::Gap),
            (0usize..10).prop_map(Zone::Ring),
            (0usize..6).prop_map(Zone::Sub),
            (0usize..10).prop_map(|w| Zone::Near { wedge: w }),
            (0.0f32..300.0).prop_map(|d| Zone::Outside { dist: d }),
        ]
    }

    fn event() -> impl Strategy<Value = Event> {
        let wheel = prop_oneof![
            Just(Wheel::Convert),
            Just(Wheel::Tools),
            Just(Wheel::Actions)
        ];
        let key = prop_oneof![
            Just(Key::Next),
            Just(Key::Prev),
            Just(Key::Up),
            Just(Key::Down),
            Just(Key::Enter),
            Just(Key::Tab { back: false })
        ];
        prop_oneof![
            (wheel, proptest::bool::ANY, 0u64..5000).prop_map(|(wheel, button_held, now)| {
                Event::Trigger {
                    wheel,
                    button_held,
                    now,
                }
            }),
            (0usize..3, 0u64..5000).prop_map(|(files, now)| Event::DragEnter { files, now }),
            (0u64..5000).prop_map(|now| Event::DragLeave { now }),
            (zone(), 0u64..5000).prop_map(|(zone, now)| Event::Pointer { zone, now }),
            (zone(), 0u64..5000).prop_map(|(zone, now)| Event::Drop { zone, now }),
            (0u64..5000).prop_map(|now| Event::ButtonUp { now }),
            Just(Event::Escape),
            (0usize..3, 0u64..5000).prop_map(|(files, now)| Event::Shortcut { files, now }),
            key.prop_map(Event::Key),
            (0u64..5000).prop_map(|now| Event::JobAccepted { now }),
            (0u64..10000).prop_map(|now| Event::Tick { now }),
        ]
    }

    proptest! {
        #[test]
        fn invariants(events in proptest::collection::vec(event(), 0..60)) {
            let mut m = Machine::new();
            for ev in events {
                let fx = m.step(ev);
                if fx.iter().any(|f| matches!(f, Effect::ShowWheel(_) | Effect::SwitchWheel(_))) {
                    m.set_layout(layout());
                }
                // Exactly one drop reply per drop.
                let replies = fx.iter().filter(|f| matches!(f, Effect::DropResult(_))).count();
                prop_assert_eq!(replies, usize::from(matches!(ev, Event::Drop { .. })));
                // Commits always name a real, committable wedge (or a preset of a real one).
                for f in &fx {
                    if let Effect::Commit { wedge, preset, .. } = *f {
                        prop_assert!(wedge < m.layout().len());
                        if preset.is_none() { prop_assert!(m.layout()[wedge].committable); }
                    }
                }
                if ev == Event::Escape {
                    prop_assert!(matches!(m.state(), State::Idle | State::Committing));
                }
            }
            // Whatever happened, Escape plus a late tick always returns to Idle.
            m.step(Event::Escape);
            m.step(Event::JobAccepted { now: 20_000 });
            m.step(Event::Tick { now: 1_000_000 });
            prop_assert_eq!(*m.state(), State::Idle);
        }
    }
}
