//! When each machine saw the user, written as text onto the running entry.
//!
//! The only place on a Harvest entry that holds text of an app's own is its
//! `external_reference`, and its `id` keeps about 250 characters. So the text is
//! terse: a version, a base instant in whole minutes, then each machine's stretches
//! as minute offsets from that base, every number in base 36. Seen at one machine
//! from nine to ten and eleven to noon, and at another from ten to eleven:
//!
//! ```text
//! 1;tq3c0;a7bq:1o-3c;k3x9:0-1o.3c-50
//! ```
//!
//! A stretch's start is rounded down to its minute and its end up, so rounding can
//! only add presence. That makes a prompt less likely by at most two minutes, well
//! inside the threshold.

use std::collections::BTreeMap;

use jiff::Timestamp;

use super::Interval;
use crate::models::ExternalReference;

/// What marks a reference as Sheaves' to rewrite. Any other is left alone.
pub const GROUP: &str = "sheaves-presence";

/// Where the reference points. Harvest requires an http(s) URL, and shows the
/// icon of its host on the entry.
pub const PERMALINK: &str =
    "https://github.com/rainhead/sheaves/blob/main/docs/presence.md#other-machines";

/// The most text Harvest was seen to keep exactly; 2,000 characters failed.
pub const MAX_LEN: usize = 250;

const VERSION: &str = "1";

/// Each machine's stretches, by machine id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Board {
    pub parts: BTreeMap<String, Vec<Interval>>,
}

/// Whether `id` can name a machine: one to eight lowercase letters and digits, so
/// it can never contain the text's separators.
pub fn is_machine_id(id: &str) -> bool {
    (1..=8).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

impl Board {
    /// The board a reference holds: empty for an entry with none, `None` for a
    /// reference that is someone else's. One of Sheaves' that does not read is
    /// treated as empty, so the next write replaces it.
    pub fn of(reference: Option<&ExternalReference>) -> Option<Board> {
        let Some(reference) = reference else {
            return Some(Board::default());
        };
        if reference.group_id.as_deref() != Some(GROUP) {
            // Harvest answers a reference with no id or permalink as one with
            // nothing in it.
            let is_empty = reference.id.is_none()
                && reference.group_id.is_none()
                && reference.permalink.is_none();
            return is_empty.then(Board::default);
        }
        Some(
            reference
                .id
                .as_deref()
                .and_then(Board::decode)
                .unwrap_or_default(),
        )
    }

    /// The reference to write for this board.
    pub fn reference(&self) -> ExternalReference {
        ExternalReference {
            id: Some(self.encode()),
            group_id: Some(GROUP.to_string()),
            permalink: Some(PERMALINK.to_string()),
        }
    }

    /// Every stretch but those of `machine`.
    pub fn others(&self, machine: &str) -> Vec<Interval> {
        self.parts
            .iter()
            .filter(|(id, _)| id.as_str() != machine)
            .flat_map(|(_, stretches)| stretches.iter().copied())
            .collect()
    }

    /// The text, no longer than [`MAX_LEN`]: while it is longer, the oldest
    /// stretch goes, whichever machine saw it, and a machine left with none goes
    /// too. A machine id that could not be read back is left out.
    pub fn encode(&self) -> String {
        let mut parts: BTreeMap<&str, Vec<(i64, i64)>> = self
            .parts
            .iter()
            .filter(|(id, _)| is_machine_id(id))
            .map(|(id, stretches)| {
                let mut minutes: Vec<(i64, i64)> = stretches
                    .iter()
                    .filter(|stretch| stretch.start <= stretch.end)
                    .map(|stretch| (floor_minute(stretch.start), ceil_minute(stretch.end)))
                    .collect();
                minutes.sort();
                (id.as_str(), minutes)
            })
            .filter(|(_, minutes)| !minutes.is_empty())
            .collect();
        loop {
            let text = render(&parts);
            if text.len() <= MAX_LEN {
                return text;
            }
            // The oldest stretch on the board, by its end, then the machine
            // holding it.
            let Some(oldest) = parts
                .iter()
                .filter_map(|(id, minutes)| minutes.first().map(|first| (first.1, *id)))
                .min()
                .map(|(_, id)| id)
            else {
                return text;
            };
            let minutes = parts.get_mut(oldest).expect("found above");
            minutes.remove(0);
            if minutes.is_empty() {
                parts.remove(oldest);
            }
        }
    }

    /// Reads text that [`Board::encode`] wrote, or `None` for anything else.
    pub fn decode(text: &str) -> Option<Board> {
        let mut fields = text.split(';');
        if fields.next()? != VERSION {
            return None;
        }
        let base = fields
            .next()
            .map(|base| i64::from_str_radix(base, 36))?
            .ok()?;
        let mut parts = BTreeMap::new();
        for field in fields {
            let (id, stretches) = field.split_once(':')?;
            if !is_machine_id(id) {
                return None;
            }
            let mut read = Vec::new();
            for stretch in stretches.split('.') {
                let (start, end) = stretch.split_once('-')?;
                let start = base.checked_add(i64::from_str_radix(start, 36).ok()?)?;
                let end = base.checked_add(i64::from_str_radix(end, 36).ok()?)?;
                if end < start {
                    return None;
                }
                read.push(Interval {
                    start: instant(start)?,
                    end: instant(end)?,
                });
            }
            parts.insert(id.to_string(), read);
        }
        Some(Board { parts })
    }
}

fn render(parts: &BTreeMap<&str, Vec<(i64, i64)>>) -> String {
    let base = parts
        .values()
        .filter_map(|minutes| minutes.first().map(|first| first.0))
        .min()
        .unwrap_or(0);
    let mut text = format!("{VERSION};{}", radix36(base));
    for (id, minutes) in parts {
        let stretches: Vec<String> = minutes
            .iter()
            .map(|(start, end)| format!("{}-{}", radix36(start - base), radix36(end - base)))
            .collect();
        text.push_str(&format!(";{id}:{}", stretches.join(".")));
    }
    text
}

fn floor_minute(instant: Timestamp) -> i64 {
    instant.as_second().div_euclid(60)
}

fn ceil_minute(instant: Timestamp) -> i64 {
    let floor = floor_minute(instant);
    match self::instant(floor) {
        Some(on_the_minute) if on_the_minute < instant => floor + 1,
        _ => floor,
    }
}

fn instant(minute: i64) -> Option<Timestamp> {
    Timestamp::from_second(minute.checked_mul(60)?).ok()
}

/// `value` in base 36, lowercase. Negative only for a base before 1970, which a
/// clock set wrong could give; it reads back all the same.
fn radix36(value: i64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut left = value.unsigned_abs();
    let mut digits = Vec::new();
    while left > 0 {
        digits.push(DIGITS[(left % 36) as usize]);
        left /= 36;
    }
    if value < 0 {
        digits.push(b'-');
    }
    digits.reverse();
    String::from_utf8(digits).expect("ASCII digits")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minutes: i64) -> Timestamp {
        Timestamp::from_second(1_793_000_000 / 60 * 60 + minutes * 60).unwrap()
    }

    fn span(start: i64, end: i64) -> Interval {
        Interval {
            start: at(start),
            end: at(end),
        }
    }

    fn board(parts: &[(&str, &[(i64, i64)])]) -> Board {
        Board {
            parts: parts
                .iter()
                .map(|(id, stretches)| {
                    (
                        id.to_string(),
                        stretches.iter().map(|(s, e)| span(*s, *e)).collect(),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn reads_back_what_it_wrote() {
        let written = board(&[("k3x9", &[(0, 60), (120, 180)]), ("a7bq", &[(60, 120)])]);
        assert_eq!(Board::decode(&written.encode()), Some(written));
    }

    /// The example in the module's documentation, so it stays true.
    #[test]
    fn writes_minutes_from_a_base_in_base_36() {
        let text = board(&[("k3x9", &[(0, 60), (120, 180)]), ("a7bq", &[(60, 120)])]).encode();
        let base = radix36(at(0).as_second() / 60);
        assert_eq!(text, format!("1;{base};a7bq:1o-3c;k3x9:0-1o.3c-50"));
    }

    #[test]
    fn rounds_a_stretch_outwards_to_whole_minutes() {
        let stretch = Interval {
            start: at(10) + jiff::SignedDuration::from_secs(59),
            end: at(20) + jiff::SignedDuration::from_secs(1),
        };
        let written = Board {
            parts: [("k3x9".to_string(), vec![stretch])].into(),
        };
        let read = Board::decode(&written.encode()).unwrap();
        assert_eq!(read.parts["k3x9"], [span(10, 21)]);
    }

    #[test]
    fn an_end_on_the_minute_stays_on_it() {
        let read = Board::decode(&board(&[("k3x9", &[(10, 20)])]).encode()).unwrap();
        assert_eq!(read.parts["k3x9"], [span(10, 20)]);
    }

    /// Five machines with a stretch every half hour across a day would be far
    /// too long; what is kept is the newest of everyone's.
    #[test]
    fn drops_the_oldest_stretches_until_it_fits() {
        let many: Vec<(i64, i64)> = (0..48).map(|i| (i * 30, i * 30 + 10)).collect();
        let written = board(&[("aaaa", &many), ("bbbb", &many[40..])]);
        let text = written.encode();
        assert!(text.len() <= MAX_LEN, "{} characters", text.len());
        let read = Board::decode(&text).unwrap();
        let newest = *many.last().unwrap();
        assert_eq!(read.parts["aaaa"].last(), Some(&span(newest.0, newest.1)));
        assert_eq!(read.parts["bbbb"].last(), Some(&span(newest.0, newest.1)));
        // Oldest first: every stretch kept ends no earlier than any that went.
        let ends = |board: &Board| -> Vec<Timestamp> {
            let mut ends: Vec<Timestamp> = board
                .parts
                .values()
                .flatten()
                .map(|stretch| stretch.end)
                .collect();
            ends.sort();
            ends
        };
        let (all, kept) = (ends(&written), ends(&read));
        let dropped = all.len() - kept.len();
        assert!(dropped > 0);
        assert!(all[..dropped].iter().all(|end| *end <= kept[0]));
    }

    #[test]
    fn a_machine_left_with_nothing_is_left_out() {
        let text = board(&[("k3x9", &[])]).encode();
        assert_eq!(Board::decode(&text), Some(Board::default()));
    }

    #[test]
    fn refuses_text_it_did_not_write() {
        for text in [
            "",
            "2;0;k3x9:0-1",
            "1",
            "1;0;K3X9:0-1",
            "1;0;k3x9:2-1",
            "1;0;k3x9:0",
            "1;0;k3x9",
            "1;0;k3x9:0-1;",
            "1;0;toolongname:0-1",
            "1;zzzzzzzzzzzzzz;k3x9:0-1",
        ] {
            assert_eq!(Board::decode(text), None, "{text:?}");
        }
    }

    #[test]
    fn an_empty_board_reads_back() {
        assert_eq!(
            Board::decode(&Board::default().encode()),
            Some(Board::default())
        );
    }

    mod references {
        use super::*;

        fn reference(group: Option<&str>, id: Option<&str>) -> ExternalReference {
            ExternalReference {
                id: id.map(str::to_string),
                group_id: group.map(str::to_string),
                permalink: Some(PERMALINK.to_string()),
            }
        }

        #[test]
        fn an_entry_with_none_is_an_empty_board() {
            assert_eq!(Board::of(None), Some(Board::default()));
            assert_eq!(
                Board::of(Some(&ExternalReference::default())),
                Some(Board::default())
            );
        }

        #[test]
        fn someone_elses_is_not_ours_to_rewrite() {
            let theirs = reference(Some("PROJ"), Some("PROJ-12"));
            assert_eq!(Board::of(Some(&theirs)), None);
            assert_eq!(
                Board::of(Some(&reference(None, Some("1;0;k3x9:0-1")))),
                None
            );
        }

        #[test]
        fn ours_is_read() {
            let written = board(&[("k3x9", &[(0, 5)])]);
            assert_eq!(Board::of(Some(&written.reference())), Some(written));
        }

        #[test]
        fn ours_garbled_is_empty_so_the_next_write_replaces_it() {
            assert_eq!(
                Board::of(Some(&reference(Some(GROUP), Some("nonsense")))),
                Some(Board::default())
            );
        }
    }

    #[test]
    fn others_leaves_out_the_machine_asking() {
        let both = board(&[("k3x9", &[(0, 5)]), ("a7bq", &[(10, 15)])]);
        assert_eq!(both.others("k3x9"), [span(10, 15)]);
    }

    #[test]
    fn knows_a_machine_id() {
        assert!(is_machine_id("k3x9"));
        for bad in ["", "K3X9", "k3-9", "k3:9", "123456789"] {
            assert!(!is_machine_id(bad), "{bad:?}");
        }
    }
}
