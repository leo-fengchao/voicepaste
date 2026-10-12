//! Hotkey chord-matching state machine over the raw keytap event stream.

use keytap::{EventKind, Key};
use std::collections::HashSet;

use super::{HotkeyBinding, HotkeyMode};

/// Actions emitted by [`MatcherState::process`]. The listener spawns these on
/// the async runtime; lib.rs decides whether a `StartRecording` actually
/// records (or is diverted to a retry) and resets the matcher when it doesn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Begin a neutral recording — the prompt is decided later, on stop.
    /// `is_main` is true when the triggering chord is the main hotkey
    /// (prompt_id `None`), which gates the "press main hotkey to retry the
    /// last failure" shortcut.
    StartRecording { is_main: bool },
    /// End the recording and finalize with the given prompt (`None` = raw
    /// paste, no polishing).
    StopRecording { prompt_id: Option<String> },
}

/// Pure state machine over the raw keytap event stream.
///
/// Resolves chord conflicts without any timeout/pending:
/// - A **hold** chord starts recording the instant its keys are all pressed
///   (keydown) — zero latency, prompt undecided.
/// - A **toggle** chord starts/stops on completion of a press cycle (the held
///   set empties at keyup). Only the **main** toggle chord (prompt_id `None`)
///   wakes recording; toggle prompt chords are stop-only — they finalize and
///   polish when pressed during a recording. The prompt is the longest chord
///   reached in that stop cycle.
/// - Recording always starts *neutral*; the prompt is fixed only when it ends,
///   from the longest chord held during the hold (peak) or the stop cycle.
///   The chord is fully known at that point, so prefix conflicts like `Ctrl`
///   vs `Ctrl+Shift` resolve correctly with no waiting.
#[derive(Debug, Default)]
pub struct MatcherState {
    /// Currently physically held keys.
    pub(super) held: HashSet<Key>,
    /// Longest binding matched during the current press cycle / hold (peak).
    cycle_best: Option<usize>,
    /// What the matcher believes is recording, if anything. Authoritative on
    /// the hotkey side; lib.rs resets it via [`MatcherState::reset_recording`]
    /// when a start is diverted to retry or fails, or when recording cancels.
    recording: Option<HotkeyMode>,
    /// For hold: the binding that started the session — its keys leaving the
    /// held set ends the session.
    active_hold: Option<usize>,
}

impl MatcherState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all matcher-side state after the app ended a recording out-of-band.
    pub fn reset_recording(&mut self) {
        self.held.clear();
        self.recording = None;
        self.cycle_best = None;
        self.active_hold = None;
    }

    /// Advance the machine one event. Returns the actions to dispatch.
    ///
    /// The caller (the listener) filters events via
    /// [`is_relevant_hotkey_event`], so `held` only ever tracks keys that are
    /// part of some binding, plus modifiers.
    pub fn process(&mut self, kind: EventKind, bindings: &[HotkeyBinding]) -> Vec<HotkeyAction> {
        let mut actions = Vec::new();
        match kind {
            EventKind::KeyDown(k) => {
                self.held.insert(k);
                self.update_cycle_best(bindings);
                // Hold starts on keydown the moment its chord completes.
                if self.recording.is_none() {
                    if let Some(idx) = longest_match(&self.held, bindings) {
                        if bindings[idx].mode == HotkeyMode::Hold {
                            let is_main = bindings[idx].prompt_id.is_none();
                            self.recording = Some(HotkeyMode::Hold);
                            self.active_hold = Some(idx);
                            actions.push(HotkeyAction::StartRecording { is_main });
                        }
                    }
                }
            }
            EventKind::KeyUp(k) => {
                self.held.remove(&k);
                // No update_cycle_best here: cycle_best only grows, and a KeyUp
                // shrinks `held`, so it could never produce a longer match — the
                // call was a guaranteed no-op.
                match self.recording {
                    Some(HotkeyMode::Hold) => {
                        // End when the active hold chord is no longer fully held.
                        let still_held = self
                            .active_hold
                            .map(|i| bindings[i].keys.iter().all(|key| self.held.contains(key)))
                            .unwrap_or(false);
                        if !still_held {
                            let prompt_id = self
                                .cycle_best
                                .and_then(|i| bindings.get(i))
                                .and_then(|b| b.prompt_id.clone());
                            self.reset_recording();
                            actions.push(HotkeyAction::StopRecording { prompt_id });
                        }
                    }
                    Some(HotkeyMode::Toggle) => {
                        if self.held.is_empty() {
                            // Stop only if this cycle reached a toggle chord.
                            if let Some(i) = self
                                .cycle_best
                                .filter(|&i| bindings[i].mode == HotkeyMode::Toggle)
                            {
                                let prompt_id = bindings.get(i).and_then(|b| b.prompt_id.clone());
                                self.reset_recording();
                                actions.push(HotkeyAction::StopRecording { prompt_id });
                            } else {
                                // Only irrelevant keys pressed — keep recording,
                                // begin a fresh stop cycle.
                                self.cycle_best = None;
                            }
                        }
                    }
                    None => {
                        if self.held.is_empty() {
                            // Start cycle: in hands-free (toggle) mode only the main
                            // hotkey (prompt_id None) wakes recording; toggle prompt
                            // chords are stop-only — they finalize + polish once a
                            // recording is already running.
                            if self
                                .cycle_best
                                .filter(|&i| bindings[i].mode == HotkeyMode::Toggle)
                                .filter(|&i| bindings[i].prompt_id.is_none())
                                .is_some()
                            {
                                self.recording = Some(HotkeyMode::Toggle);
                                actions.push(HotkeyAction::StartRecording { is_main: true });
                            }
                            self.cycle_best = None;
                        }
                    }
                }
            }
            EventKind::KeyRepeat(_) => {}
        }
        actions
    }

    /// Track the longest binding matched since the cycle/hold began. Only grows
    /// (strictly longer) so it holds the peak across a hold.
    fn update_cycle_best(&mut self, bindings: &[HotkeyBinding]) {
        if let Some(idx) = longest_match(&self.held, bindings) {
            let take = self
                .cycle_best
                .map(|prev| bindings[idx].keys.len() > bindings[prev].keys.len())
                .unwrap_or(true);
            if take {
                self.cycle_best = Some(idx);
            }
        }
    }

    /// Whether a fresh press of `key` should be swallowed instead of reaching
    /// the focused app. True only while a recording is active and `key`
    /// completes a registered chord (every other key of that chord is already
    /// held, and no modifier outside it): the press is then a command to the
    /// recording, not typing. While idle nothing is swallowed, so a plain key
    /// bound as a stop-only prompt hotkey still types normally.
    pub(super) fn should_suppress(&self, key: Key, bindings: &[HotkeyBinding]) -> bool {
        self.recording.is_some()
            && bindings.iter().any(|b| {
                b.keys.contains(&key)
                    && b.keys.iter().all(|k| *k == key || self.held.contains(k))
                    && no_foreign_modifier(&self.held, b)
            })
    }

    /// Whether a press of `key` must not reach the foreground app at all —
    /// the decision the keytap suppress predicate runs on every key event.
    /// Layered on the same exact-match semantics as [`should_suppress`]:
    ///
    /// 1. Modifiers never swallow: they carry no character of their own and
    ///    swallowing one corrupts the system modifier state.
    /// 2. A key of the *main* binding swallows whenever pressed with no
    ///    foreign modifier held — a main-hotkey press is always a command
    ///    (start or stop), so its character must never leak into the focused
    ///    app. This covers the press that *starts* a recording, which
    ///    [`should_suppress`] cannot: the recording flag only rises after the
    ///    key's release (toggle) or asynchronously after the press (hold).
    /// 3. Otherwise [`should_suppress`]: a prompt stop key completing its
    ///    chord while a recording runs.
    pub(super) fn should_swallow(&self, key: Key, bindings: &[HotkeyBinding]) -> bool {
        if EXACT_MODIFIERS.contains(&key) {
            return false;
        }
        if bindings.iter().any(|b| {
            b.prompt_id.is_none() && b.keys.contains(&key) && no_foreign_modifier(&self.held, b)
        }) {
            return true;
        }
        self.should_suppress(key, bindings)
    }

    /// Reclaim `held` keys stuck down by a lost keyup. The listener calls this
    /// from its idle tick, and only while idle (`recording` is None), so it
    /// never interferes with an active recording — hold-to-talk or a toggle
    /// session mid-pause are both left untouched.
    pub fn clear_stale_held(&mut self) {
        if self.recording.is_none() && !self.held.is_empty() {
            self.held.clear();
            self.cycle_best = None;
        }
    }
}

/// Modifiers that must match a binding exactly. `Fn` is deliberately absent:
/// on laptop keyboards it is how the F-row keys are physically reached.
const EXACT_MODIFIERS: &[Key] = &[
    Key::ControlLeft,
    Key::ControlRight,
    Key::AltLeft,
    Key::AltRight,
    Key::ShiftLeft,
    Key::ShiftRight,
    Key::MetaLeft,
    Key::MetaRight,
];

/// Whether every held modifier belongs to `binding`. A hotkey `F` must not
/// fire for `Ctrl+F` — that is the focused app's shortcut, not ours.
fn no_foreign_modifier(held: &HashSet<Key>, binding: &HotkeyBinding) -> bool {
    held.iter()
        .all(|k| !EXACT_MODIFIERS.contains(k) || binding.keys.contains(k))
}

/// Longest-match resolution: of all bindings whose keys are a subset of `held`
/// with no foreign modifier held, the one with the most keys wins; ties are
/// broken by registration order (earlier wins). Replaces the old
/// first-subset-match that let a bare `Ctrl` steal `Ctrl+Shift`.
pub fn longest_match(held: &HashSet<Key>, bindings: &[HotkeyBinding]) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for (i, b) in bindings.iter().enumerate() {
        if !b.keys.is_empty()
            && b.keys.iter().all(|k| held.contains(k))
            && no_foreign_modifier(held, b)
        {
            let len = b.keys.len();
            let take = match best {
                None => true,
                Some((_, best_len)) => len > best_len,
            };
            if take {
                best = Some((i, len));
            }
        }
    }
    best.map(|(i, _)| i)
}

fn hotkey_event_key(kind: EventKind) -> Option<Key> {
    match kind {
        EventKind::KeyDown(key) | EventKind::KeyUp(key) | EventKind::KeyRepeat(key) => Some(key),
    }
}

fn key_is_registered_hotkey_part(key: Key, bindings: &[HotkeyBinding]) -> bool {
    bindings.iter().any(|binding| binding.keys.contains(&key))
}

/// Whether an event concerns a key the matcher must track: a key that is part
/// of some registered hotkey binding, or any modifier (needed to reject chords
/// with a foreign modifier held). The listener drops everything else at the
/// boundary so keys like CapsLock or NumLock — whose keyup the OS often fails
/// to deliver — can never enter the matcher's `held` set and jam the toggle
/// cycle.
pub(super) fn is_relevant_hotkey_event(kind: EventKind, bindings: &[HotkeyBinding]) -> bool {
    hotkey_event_key(kind)
        .map(|k| EXACT_MODIFIERS.contains(&k) || key_is_registered_hotkey_part(k, bindings))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use keytap::{EventKind, Key};

    fn down(k: Key) -> EventKind {
        EventKind::KeyDown(k)
    }
    fn up(k: Key) -> EventKind {
        EventKind::KeyUp(k)
    }

    /// Drive the matcher through an event sequence, collecting emitted actions.
    fn run_matcher(bindings: &[HotkeyBinding], events: &[EventKind]) -> Vec<HotkeyAction> {
        let mut m = MatcherState::new();
        run_matcher_from(&mut m, bindings, events)
    }

    fn run_matcher_from(
        matcher: &mut MatcherState,
        bindings: &[HotkeyBinding],
        events: &[EventKind],
    ) -> Vec<HotkeyAction> {
        let mut out = Vec::new();
        for e in events {
            out.extend(matcher.process(*e, bindings));
        }
        out
    }

    fn binding(keys: &[Key], mode: &str, prompt_id: Option<&str>) -> HotkeyBinding {
        HotkeyBinding {
            keys: keys.iter().copied().collect(),
            mode: HotkeyMode::from_str(mode),
            prompt_id: prompt_id.map(str::to_string),
        }
    }

    #[test]
    fn toggle_main_cycle_starts_neutral_then_stops_raw() {
        let bindings = [binding(&[Key::ControlLeft], "toggle", None)];
        let actions = run_matcher(
            &bindings,
            &[
                down(Key::ControlLeft),
                up(Key::ControlLeft),
                down(Key::ControlLeft),
                up(Key::ControlLeft),
            ],
        );
        assert_eq!(
            actions,
            vec![
                HotkeyAction::StartRecording { is_main: true },
                HotkeyAction::StopRecording { prompt_id: None },
            ]
        );
    }

    #[test]
    fn toggle_prefix_conflict_stop_chord_decides_prompt() {
        let bindings = [
            binding(&[Key::ControlLeft], "toggle", None),
            binding(
                &[Key::ControlLeft, Key::ShiftLeft],
                "toggle",
                Some("polish"),
            ),
        ];
        let actions = run_matcher(
            &bindings,
            &[
                down(Key::ControlLeft),
                up(Key::ControlLeft),
                down(Key::ControlLeft),
                down(Key::ShiftLeft),
                up(Key::ShiftLeft),
                up(Key::ControlLeft),
            ],
        );
        assert_eq!(
            actions,
            vec![
                HotkeyAction::StartRecording { is_main: true },
                HotkeyAction::StopRecording {
                    prompt_id: Some("polish".to_string())
                },
            ]
        );
    }

    #[test]
    fn toggle_main_only_wakes_prompt_chord_is_stop_only() {
        let bindings = [
            binding(&[Key::ControlLeft], "toggle", None),
            binding(
                &[Key::ControlLeft, Key::ShiftLeft],
                "toggle",
                Some("polish"),
            ),
        ];
        // Hands-free wake is main-only: pressing the longer prompt chord
        // (Ctrl+Shift) while idle does NOT start. The main chord (Ctrl) then
        // wakes, and pressing it again stops raw.
        let actions = run_matcher(
            &bindings,
            &[
                down(Key::ControlLeft),
                down(Key::ShiftLeft),
                up(Key::ShiftLeft),
                up(Key::ControlLeft),
                down(Key::ControlLeft),
                up(Key::ControlLeft),
                down(Key::ControlLeft),
                up(Key::ControlLeft),
            ],
        );
        assert_eq!(
            actions,
            vec![
                HotkeyAction::StartRecording { is_main: true },
                HotkeyAction::StopRecording { prompt_id: None },
            ]
        );
    }

    #[test]
    fn toggle_prompt_chord_does_not_start_when_idle() {
        // A toggle prompt chord pressed while idle must never wake recording —
        // only the main hotkey does. The prompt chord only acts on stop.
        let bindings = [
            binding(&[Key::F13], "toggle", None),
            binding(
                &[Key::ControlLeft, Key::ShiftLeft],
                "toggle",
                Some("polish"),
            ),
        ];
        let actions = run_matcher(
            &bindings,
            &[
                down(Key::ControlLeft),
                down(Key::ShiftLeft),
                up(Key::ShiftLeft),
                up(Key::ControlLeft),
                down(Key::F13),
                up(Key::F13),
            ],
        );
        assert_eq!(
            actions,
            vec![HotkeyAction::StartRecording { is_main: true }]
        );
    }

    #[test]
    fn hold_starts_on_keydown_stops_on_release() {
        let bindings = [binding(&[Key::ControlLeft], "hold", None)];
        let actions = run_matcher(&bindings, &[down(Key::ControlLeft), up(Key::ControlLeft)]);
        assert_eq!(
            actions,
            vec![
                HotkeyAction::StartRecording { is_main: true },
                HotkeyAction::StopRecording { prompt_id: None },
            ]
        );
    }

    #[test]
    fn hold_prefix_conflict_uses_peak_for_prompt() {
        let bindings = [
            binding(&[Key::ControlLeft], "hold", None),
            binding(&[Key::ControlLeft, Key::ShiftLeft], "hold", Some("polish")),
        ];
        let actions = run_matcher(
            &bindings,
            &[
                down(Key::ControlLeft),
                down(Key::ShiftLeft),
                up(Key::ShiftLeft),
                up(Key::ControlLeft),
            ],
        );
        assert_eq!(
            actions,
            vec![
                HotkeyAction::StartRecording { is_main: true },
                HotkeyAction::StopRecording {
                    prompt_id: Some("polish".to_string())
                },
            ]
        );
    }

    #[test]
    fn hold_short_only_yields_raw_prompt() {
        let bindings = [
            binding(&[Key::ControlLeft], "hold", None),
            binding(&[Key::ControlLeft, Key::ShiftLeft], "hold", Some("polish")),
        ];
        let actions = run_matcher(&bindings, &[down(Key::ControlLeft), up(Key::ControlLeft)]);
        assert_eq!(
            actions,
            vec![
                HotkeyAction::StartRecording { is_main: true },
                HotkeyAction::StopRecording { prompt_id: None },
            ]
        );
    }

    #[test]
    fn mixed_toggle_short_hold_long_no_conflict() {
        let bindings = [
            binding(&[Key::ControlLeft], "toggle", None),
            binding(&[Key::ControlLeft, Key::ShiftLeft], "hold", Some("polish")),
        ];
        let actions = run_matcher(
            &bindings,
            &[
                down(Key::ControlLeft),
                down(Key::ShiftLeft),
                up(Key::ShiftLeft),
                up(Key::ControlLeft),
            ],
        );
        assert_eq!(
            actions,
            vec![
                HotkeyAction::StartRecording { is_main: false },
                HotkeyAction::StopRecording {
                    prompt_id: Some("polish".to_string())
                },
            ]
        );
    }

    #[test]
    fn toggle_irrelevant_key_during_recording_does_not_stop() {
        let bindings = [binding(&[Key::F13], "toggle", None)];
        let actions = run_matcher(
            &bindings,
            &[
                down(Key::F13),
                up(Key::F13),
                down(Key::Space),
                up(Key::Space),
                down(Key::F13),
                up(Key::F13),
            ],
        );
        assert_eq!(
            actions,
            vec![
                HotkeyAction::StartRecording { is_main: true },
                HotkeyAction::StopRecording { prompt_id: None },
            ]
        );
    }

    #[test]
    fn longest_match_prefers_more_keys_then_registration_order() {
        let bindings = [
            binding(&[Key::A], "toggle", None),         // idx 0, len 1
            binding(&[Key::A, Key::B], "toggle", None), // idx 1, len 2
            binding(&[Key::A, Key::C], "toggle", None), // idx 2, len 2 (tie with 1)
        ];
        // A+B and A+C both match with len 2; earlier registration (idx 1) wins.
        let held: HashSet<Key> = [Key::A, Key::B, Key::C].into_iter().collect();
        assert_eq!(longest_match(&held, &bindings), Some(1));

        let held: HashSet<Key> = [Key::A, Key::C].into_iter().collect();
        assert_eq!(longest_match(&held, &bindings), Some(2));

        let held: HashSet<Key> = [Key::A].into_iter().collect();
        assert_eq!(longest_match(&held, &bindings), Some(0));
    }

    #[test]
    fn reset_recording_clears_session_tracking() {
        let bindings = [binding(&[Key::F13], "hold", None)];
        let mut m = MatcherState::new();
        let started = m.process(down(Key::F13), &bindings);
        assert_eq!(started.len(), 1); // StartRecording emitted → session active
        m.reset_recording();
        // After reset, releasing the key must not emit a stray StopRecording
        // (the session was cancelled out-of-band).
        let actions = m.process(up(Key::F13), &bindings);
        assert!(actions.is_empty());
    }

    #[test]
    fn reset_recording_clears_stale_held_keys() {
        let bindings = [binding(&[Key::F13], "toggle", None)];
        let mut m = MatcherState::new();

        // Simulate a lost keyup for an unrelated key. If reset leaves physical
        // held state behind, the next toggle cycle never sees held.is_empty().
        assert!(m.process(down(Key::Space), &bindings).is_empty());
        assert!(m
            .process(down(Key::F13), &bindings)
            .into_iter()
            .chain(m.process(up(Key::F13), &bindings))
            .collect::<Vec<_>>()
            .is_empty());

        m.reset_recording();
        let actions = run_matcher_from(&mut m, &bindings, &[down(Key::F13), up(Key::F13)]);
        assert_eq!(
            actions,
            vec![HotkeyAction::StartRecording { is_main: true }]
        );
    }

    #[test]
    fn allowlist_drops_events_for_unregistered_keys() {
        // Only keys that are part of a registered binding are relevant; anything
        // else (CapsLock, NumLock, plain typing) is dropped at the listener
        // boundary so it can never jam the matcher's held set.
        let bindings = [binding(&[Key::Function], "toggle", None)];
        assert!(!is_relevant_hotkey_event(down(Key::CapsLock), &bindings));
        assert!(!is_relevant_hotkey_event(down(Key::NumLock), &bindings));
        assert!(!is_relevant_hotkey_event(down(Key::Space), &bindings));
        assert!(is_relevant_hotkey_event(down(Key::Function), &bindings));
        assert!(is_relevant_hotkey_event(up(Key::Function), &bindings));
    }

    #[test]
    fn clear_stale_held_only_while_idle() {
        let bindings = [binding(&[Key::Function], "hold", None)];
        let mut m = MatcherState::new();

        // Start a hold session: Function is held and the matcher is recording.
        assert_eq!(
            m.process(down(Key::Function), &bindings),
            vec![HotkeyAction::StartRecording { is_main: true }]
        );
        // While recording, stale reclaim must NOT touch held.
        m.clear_stale_held();
        assert_eq!(
            m.process(up(Key::Function), &bindings),
            vec![HotkeyAction::StopRecording { prompt_id: None }]
        );

        // Idle again with a stuck key (a lost keyup): reclaim clears it.
        m.held.insert(Key::Function);
        m.clear_stale_held();
        assert!(m.held.is_empty());
    }

    #[test]
    fn foreign_modifier_blocks_plain_stop_key() {
        let bindings = [
            binding(&[Key::F13], "toggle", None),
            binding(&[Key::F], "toggle", Some("translate")),
        ];
        let mut m = MatcherState::new();
        run_matcher_from(&mut m, &bindings, &[down(Key::F13), up(Key::F13)]);
        // Ctrl+F belongs to the focused app: the recording keeps running.
        let actions = run_matcher_from(
            &mut m,
            &bindings,
            &[
                down(Key::ControlLeft),
                down(Key::F),
                up(Key::F),
                up(Key::ControlLeft),
            ],
        );
        assert!(actions.is_empty());
        // Releasing Ctrl first must not turn the still-held F into a match.
        let actions = run_matcher_from(
            &mut m,
            &bindings,
            &[
                down(Key::ControlLeft),
                down(Key::F),
                up(Key::ControlLeft),
                up(Key::F),
            ],
        );
        assert!(actions.is_empty());
        // A bare F still stops with its prompt.
        let actions = run_matcher_from(&mut m, &bindings, &[down(Key::F), up(Key::F)]);
        assert_eq!(
            actions,
            vec![HotkeyAction::StopRecording {
                prompt_id: Some("translate".to_string())
            }]
        );
    }

    #[test]
    fn foreign_modifier_blocks_start() {
        let toggle = [binding(&[Key::F13], "toggle", None)];
        let events = [
            down(Key::ShiftLeft),
            down(Key::F13),
            up(Key::F13),
            up(Key::ShiftLeft),
        ];
        assert!(run_matcher(&toggle, &events).is_empty());
        let hold = [binding(&[Key::F13], "hold", None)];
        assert!(run_matcher(&hold, &events).is_empty());
    }

    #[test]
    fn modifiers_are_always_relevant_events() {
        let bindings = [binding(&[Key::F], "toggle", None)];
        assert!(is_relevant_hotkey_event(down(Key::ControlLeft), &bindings));
        assert!(is_relevant_hotkey_event(up(Key::MetaRight), &bindings));
        assert!(!is_relevant_hotkey_event(down(Key::G), &bindings));
        assert!(!is_relevant_hotkey_event(down(Key::CapsLock), &bindings));
    }

    #[test]
    fn suppress_plain_stop_key_only_while_recording() {
        let bindings = [
            binding(&[Key::F13], "toggle", None),
            binding(&[Key::F], "toggle", Some("translate")),
        ];
        let mut m = MatcherState::new();
        // Idle: F is ordinary typing.
        assert!(!m.should_suppress(Key::F, &bindings));

        run_matcher_from(&mut m, &bindings, &[down(Key::F13), up(Key::F13)]);
        assert!(m.should_suppress(Key::F, &bindings));
        assert!(m.should_suppress(Key::F13, &bindings));
        // Keys outside every binding are never swallowed.
        assert!(!m.should_suppress(Key::G, &bindings));
        // Still true once the listener has already seen this keydown.
        run_matcher_from(&mut m, &bindings, &[down(Key::F)]);
        assert!(m.should_suppress(Key::F, &bindings));

        // The stop cycle ends the recording; F types again.
        run_matcher_from(&mut m, &bindings, &[up(Key::F)]);
        assert!(!m.should_suppress(Key::F, &bindings));
    }

    #[test]
    fn suppress_chord_key_requires_rest_of_chord_held() {
        let bindings = [
            binding(&[Key::F13], "toggle", None),
            binding(&[Key::ControlLeft, Key::F], "toggle", Some("translate")),
        ];
        let mut m = MatcherState::new();
        run_matcher_from(&mut m, &bindings, &[down(Key::F13), up(Key::F13)]);
        // Bare F does not complete Ctrl+F — it is dictation-time typing.
        assert!(!m.should_suppress(Key::F, &bindings));
        run_matcher_from(&mut m, &bindings, &[down(Key::ControlLeft)]);
        assert!(m.should_suppress(Key::F, &bindings));
    }

    #[test]
    fn suppress_skips_plain_key_with_foreign_modifier() {
        let bindings = [
            binding(&[Key::F13], "toggle", None),
            binding(&[Key::F], "toggle", Some("translate")),
        ];
        let mut m = MatcherState::new();
        run_matcher_from(&mut m, &bindings, &[down(Key::F13), up(Key::F13)]);
        run_matcher_from(&mut m, &bindings, &[down(Key::MetaLeft)]);
        // Cmd+F must reach the focused app.
        assert!(!m.should_suppress(Key::F, &bindings));
    }

    #[test]
    fn swallow_main_key_when_idle() {
        // The main hotkey's press IS the start command, so it swallows even
        // before the recording flag rises; a prompt stop key and an unrelated
        // key stay ordinary typing while idle.
        let bindings = [
            binding(&[Key::F13], "toggle", None),
            binding(&[Key::F], "toggle", Some("translate")),
        ];
        let m = MatcherState::new();
        assert!(m.should_swallow(Key::F13, &bindings));
        assert!(!m.should_swallow(Key::F, &bindings));
        assert!(!m.should_swallow(Key::G, &bindings));
    }

    #[test]
    fn swallow_never_for_modifiers() {
        let bindings = [binding(&[Key::ControlLeft, Key::F13], "toggle", None)];
        let mut m = MatcherState::new();
        assert!(!m.should_swallow(Key::ControlLeft, &bindings));
        run_matcher_from(&mut m, &bindings, &[down(Key::ControlLeft)]);
        // Even mid-chord: swallowing a modifier corrupts the system modifier
        // state. The chord's non-modifier key still swallows.
        assert!(!m.should_swallow(Key::ControlLeft, &bindings));
        assert!(m.should_swallow(Key::F13, &bindings));
    }

    #[test]
    fn swallow_main_key_blocked_by_foreign_modifier() {
        let bindings = [binding(&[Key::F13], "toggle", None)];
        let mut m = MatcherState::new();
        run_matcher_from(&mut m, &bindings, &[down(Key::MetaLeft)]);
        // Cmd+F13 belongs to the foreground app — nothing swallows.
        assert!(!m.should_swallow(Key::F13, &bindings));
    }
}
