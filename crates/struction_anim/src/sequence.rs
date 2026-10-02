//! Pose sequences: key poses played in order. A move stretches a one-shot sequence over its
//! duration (a roll, a swing); a state loops one while it holds (a walk cycle). Sampling here is
//! pure; [`crate::moves`] applies it to rigs.

use std::collections::BTreeMap;

use bevy::reflect::{Reflect, std_traits::ReflectDefault};
use serde::{Deserialize, Serialize};

use crate::base_pose::BasePoseSet;
use crate::error::AnimError;

/// Key poses played in order, with how the sequence blends with the rest of the body.
#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Default)]
#[serde(default)]
pub struct PoseSequence {
    /// What the sequence is for, shown by tools.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub doc: String,
    /// Reached in order; at least one.
    pub keys: Vec<SequenceKey>,
    /// Repeats every `seconds`, wrapping from the last key to the first, until stopped.
    #[serde(skip_serializing_if = "is_false")]
    pub looping: bool,
    /// Length of a looping cycle. Moves stretch one-shots over their own duration; tools preview
    /// them at this length.
    pub seconds: f32,
    /// Fraction of locomotion, feet and constraints replaced: 1 for the whole body, 0 leaves
    /// walking legs alone. Keys start from the rest pose in proportion.
    pub takeover: f32,
    /// Fractions of a one-shot spent blending in and out. Loops blend over [`LOOP_BLEND`] seconds.
    pub fade_in: f32,
    pub fade_out: f32,
    /// Named instants, as fractions, that simulation acts on, such as `strike`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub events: BTreeMap<String, f32>,
    /// A full turn of the pelvis across the travel direction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tumble: Option<Tumble>,
}

impl Default for PoseSequence {
    fn default() -> Self {
        Self {
            doc: String::new(),
            keys: Vec::new(),
            looping: false,
            seconds: 1.0,
            takeover: 0.0,
            fade_in: 0.1,
            fade_out: 0.2,
            events: BTreeMap::new(),
            tumble: None,
        }
    }
}

/// Seconds a looping sequence takes to blend in when its state starts, and out when it ends.
pub const LOOP_BLEND: f32 = 0.2;

fn is_false(value: &bool) -> bool {
    !value
}

/// A pose reached at a fraction of the sequence, eased from the previous key.
#[derive(Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Default)]
#[serde(default)]
pub struct SequenceKey {
    pub pose: String,
    pub at: f32,
}

impl SequenceKey {
    pub fn new(pose: impl Into<String>, at: f32) -> Self {
        Self {
            pose: pose.into(),
            at,
        }
    }
}

/// The pelvis turns once about the axis across the travel direction between `from` and `to`,
/// lowered to `sink` of its height.
#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Default)]
#[serde(default)]
pub struct Tumble {
    pub from: f32,
    pub to: f32,
    pub sink: f32,
}

impl Default for Tumble {
    fn default() -> Self {
        Self {
            from: 0.15,
            to: 0.8,
            sink: 0.62,
        }
    }
}

pub(crate) fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl PoseSequence {
    /// Authoring checks, reported with the sequence's `name`.
    pub fn validate(&self, name: &str, library: &BasePoseSet) -> Result<(), AnimError> {
        let invalid = |message: String| {
            Err(AnimError::InvalidSequence {
                sequence: name.to_owned(),
                message,
            })
        };
        let fraction = |value: f32| value.is_finite() && (0.0..=1.0).contains(&value);
        if self.keys.is_empty() {
            return invalid("needs at least one key".into());
        }
        for (index, key) in self.keys.iter().enumerate() {
            if !library.poses.contains_key(&key.pose) {
                return invalid(format!("key {index} plays unknown pose `{}`", key.pose));
            }
            if !fraction(key.at) {
                return invalid(format!("key {index} is at {}, outside 0..=1", key.at));
            }
            if index > 0 && key.at < self.keys[index - 1].at {
                return invalid(format!("key {index} comes before the key preceding it"));
            }
        }
        if !self.seconds.is_finite() || self.seconds <= 0.0 {
            return invalid("seconds must be finite and greater than zero".into());
        }
        if !fraction(self.takeover) {
            return invalid("takeover must be within 0..=1".into());
        }
        if !fraction(self.fade_in) || !fraction(self.fade_out) {
            return invalid("fade_in and fade_out must be within 0..=1".into());
        }
        for (event, at) in &self.events {
            if !fraction(*at) {
                return invalid(format!("event `{event}` is at {at}, outside 0..=1"));
            }
        }
        if let Some(tumble) = &self.tumble
            && !(fraction(tumble.from)
                && fraction(tumble.to)
                && tumble.from < tumble.to
                && tumble.sink.is_finite())
        {
            return invalid("tumble needs 0 <= from < to <= 1 and a finite sink".into());
        }
        Ok(())
    }

    /// Fraction at which the named event happens.
    pub fn event(&self, name: &str) -> Option<f32> {
        self.events.get(name).copied()
    }

    /// The phase of a looping sequence `seconds` into playing it; one-shots clamp to the end.
    pub fn phase_at(&self, seconds: f32) -> f32 {
        let phase = seconds / self.seconds;
        if self.looping {
            phase.rem_euclid(1.0)
        } else {
            phase.clamp(0.0, 1.0)
        }
    }

    /// How fully the sequence shows at `phase`: one-shots fade in and out, loops stay on.
    pub fn envelope(&self, phase: f32) -> f32 {
        if self.looping {
            return 1.0;
        }
        let fade = |t: f32, length: f32| {
            if length > 0.0 {
                smooth(t / length)
            } else {
                1.0
            }
        };
        fade(phase, self.fade_in) * fade(1.0 - phase, self.fade_out)
    }

    /// Keys to apply in order at `phase`, each with its weight. Every key up to the current one
    /// applies fully, so a joint keeps the last value it was given; the next key eases in. Before
    /// the first key a one-shot holds it; a loop eases from its last key around to its first.
    pub fn blend(&self, phase: f32) -> Vec<(usize, f32)> {
        let count = self.keys.len();
        if count == 0 {
            return Vec::new();
        }
        let current = self.keys.iter().rposition(|key| key.at <= phase);
        if !self.looping {
            let Some(current) = current else {
                return vec![(0, 1.0)];
            };
            let mut keys: Vec<_> = (0..=current).map(|index| (index, 1.0)).collect();
            if let Some(next) = self.keys.get(current + 1) {
                let span = next.at - self.keys[current].at;
                let t = if span > 0.0 {
                    smooth((phase - self.keys[current].at) / span)
                } else {
                    1.0
                };
                keys.push((current + 1, t));
            }
            return keys;
        }
        let current = current.unwrap_or(count - 1);
        let next = (current + 1) % count;
        let mut span = self.keys[next].at - self.keys[current].at;
        if span <= 0.0 {
            span += 1.0;
        }
        let elapsed = (phase - self.keys[current].at).rem_euclid(1.0);
        let mut keys: Vec<_> = (1..=count)
            .map(|offset| ((current + offset) % count, 1.0))
            .collect();
        keys.push((next, smooth(elapsed / span)));
        keys
    }

    /// The tumble's angle (radians) at `phase` and its pelvis height scale.
    pub fn tumble_at(&self, phase: f32) -> Option<(f32, f32)> {
        self.tumble.as_ref().map(|tumble| {
            let turned = smooth((phase - tumble.from) / (tumble.to - tumble.from));
            (core::f32::consts::TAU * turned, tumble.sink)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::humanoid;

    fn sequence(keys: &[(&str, f32)], looping: bool) -> PoseSequence {
        PoseSequence {
            keys: keys
                .iter()
                .map(|&(pose, at)| SequenceKey::new(pose, at))
                .collect(),
            looping,
            ..Default::default()
        }
    }

    #[test]
    fn one_shots_hold_their_first_key_then_ease_to_each_next() {
        let swing = sequence(&[("raise", 0.3), ("strike", 0.55)], false);
        assert_eq!(swing.blend(0.1), [(0, 1.0)]);
        assert_eq!(swing.blend(0.3), [(0, 1.0), (1, 0.0)]);
        let middle = swing.blend(0.425);
        assert_eq!(middle[1].0, 1);
        assert!((middle[1].1 - 0.5).abs() < 1e-5);
        assert_eq!(swing.blend(0.9), [(0, 1.0), (1, 1.0)]);
    }

    #[test]
    fn loops_play_in_order_and_wrap_from_last_to_first() {
        let walk = sequence(
            &[("one", 0.0), ("two", 1.0 / 3.0), ("three", 2.0 / 3.0)],
            true,
        );
        let order = |phase: f32| walk.blend(phase).last().unwrap().0;
        assert_eq!(order(0.1), 1);
        assert_eq!(order(0.5), 2);
        assert_eq!(order(0.9), 0);
        // Every key applies once fully, ending with the current one, before the next eases in.
        let late = walk.blend(0.9);
        assert_eq!(&late[..3], [(0, 1.0), (1, 1.0), (2, 1.0)]);
        assert!((late[3].1 - smooth(0.7)).abs() < 1e-5);
        assert_eq!(walk.phase_at(2.5 * walk.seconds), 0.5);
    }

    #[test]
    fn fades_bound_one_shots_but_not_loops() {
        let mut swing = sequence(&[("raise", 0.0)], false);
        assert_eq!(swing.envelope(0.0), 0.0);
        assert_eq!(swing.envelope(0.5), 1.0);
        assert_eq!(swing.envelope(1.0), 0.0);
        swing.looping = true;
        assert_eq!(swing.envelope(0.0), 1.0);
    }

    #[test]
    fn validation_names_the_problem() {
        let library = humanoid::base_poses();
        let mut walk = sequence(&[("idle", 0.0), ("missing", 0.5)], true);
        let error = walk.validate("walk", &library).unwrap_err().to_string();
        assert!(
            error.contains("walk") && error.contains("missing"),
            "{error}"
        );
        walk.keys[1].pose = "aim".into();
        walk.validate("walk", &library).unwrap();
        walk.keys[1].at = 1.5;
        assert!(walk.validate("walk", &library).is_err());
        walk.keys[1].at = 0.5;
        walk.seconds = 0.0;
        assert!(walk.validate("walk", &library).is_err());
    }
}
