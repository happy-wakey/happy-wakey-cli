//! Pure alarm-create boundary: explicit inputs, immutable values, typed errors.
//!
//! Side effects (UUID minting, env/flag lookup, HTTP) stay in `main`. This
//! module only transforms already-resolved values into a `CreateAlarmRequest`.

use std::collections::BTreeSet;
use std::fmt;

use happy_wakey_interfaces::CreateAlarmRequest;

const MAX_LABEL_CHARS: usize = 120;
const MAX_TIME_ZONE_CHARS: usize = 64;
const MIN_TIME_ZONE_CHARS: usize = 3;
const MAX_SOUND_CHARS: usize = 128;
const MAX_TAG_CHARS: usize = 40;
const MAX_TAGS: usize = 20;
const MAX_GRADUAL_SECONDS: u32 = 1800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftField {
    Label,
    TimeZone,
    Sound,
    Tag,
}

impl DraftField {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Label => "label",
            Self::TimeZone => "time zone",
            Self::Sound => "sound",
            Self::Tag => "tag",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlarmDraftError {
    EmptyOrControl { field: DraftField },
    TooLong { field: DraftField, max: usize },
    TimeZoneTooShort,
    LocalTimeInvalid,
    WeekdaysInvalid,
    VolumeInvalid,
    GradualSecondsInvalid,
    TooManyTags,
}

impl fmt::Display for AlarmDraftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyOrControl { field } => {
                write!(
                    f,
                    "{} is empty, too long, or contains controls",
                    field.as_str()
                )
            }
            Self::TooLong { field, .. } => {
                write!(
                    f,
                    "{} is empty, too long, or contains controls",
                    field.as_str()
                )
            }
            Self::TimeZoneTooShort => {
                write!(f, "time zone must contain at least three characters")
            }
            Self::LocalTimeInvalid => write!(f, "local time must be HH:MM or HH:MM:SS"),
            Self::WeekdaysInvalid => {
                write!(
                    f,
                    "weekdays must contain one to seven unique values from 0 through 6"
                )
            }
            Self::VolumeInvalid => write!(f, "volume must be between 0 and 1"),
            Self::GradualSecondsInvalid => write!(f, "gradual seconds must not exceed 1800"),
            Self::TooManyTags => write!(f, "at most 20 unique tags are allowed"),
        }
    }
}

impl std::error::Error for AlarmDraftError {}

/// Already-resolved create-alarm inputs. Callers mint IDs and read flags.
#[derive(Debug, Clone, PartialEq)]
pub struct AlarmDraft<'a> {
    pub transition_id: &'a str,
    pub label: &'a str,
    pub local_time: &'a str,
    pub time_zone: &'a str,
    pub weekdays: &'a [u8],
    pub enabled: bool,
    pub sound: &'a str,
    pub volume: f32,
    pub gradual_seconds: u32,
    pub tags: &'a [String],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime<'a> {
    pub hour: u8,
    pub minute: u8,
    pub second: Option<u8>,
    pub as_text: &'a str,
}

/// Validate and normalize a draft into the shared request type.
pub fn build_create_alarm_request(
    draft: AlarmDraft<'_>,
) -> Result<CreateAlarmRequest, AlarmDraftError> {
    let label = bounded_text(draft.label, MAX_LABEL_CHARS, DraftField::Label)?;
    let local_time = parse_local_time(draft.local_time)?;
    let time_zone = bounded_text(draft.time_zone, MAX_TIME_ZONE_CHARS, DraftField::TimeZone)?;
    if time_zone.len() < MIN_TIME_ZONE_CHARS {
        return Err(AlarmDraftError::TimeZoneTooShort);
    }
    let weekdays = normalize_weekdays(draft.weekdays)?;
    let sound = bounded_text(draft.sound, MAX_SOUND_CHARS, DraftField::Sound)?;
    let volume = accept_volume(draft.volume)?;
    let gradual_seconds = accept_gradual_seconds(draft.gradual_seconds)?;
    let tags = normalize_tags(draft.tags)?;
    Ok(CreateAlarmRequest {
        transition_id: draft.transition_id.to_owned(),
        label,
        local_time: local_time.as_text.to_owned(),
        time_zone,
        weekdays,
        enabled: draft.enabled,
        sound,
        volume,
        gradual_seconds,
        tags,
    })
}

/// Parse `HH:MM` or `HH:MM:SS` with an exhaustive shape match.
pub fn parse_local_time(value: &str) -> Result<LocalTime<'_>, AlarmDraftError> {
    let parts: Vec<&str> = value.split(':').collect();
    match parts.as_slice() {
        [hour, minute] => decode_clock(value, hour, minute, None),
        [hour, minute, second] => decode_clock(value, hour, minute, Some(second)),
        _ => Err(AlarmDraftError::LocalTimeInvalid),
    }
}

/// Sort, uniquify, and bound weekday numbers without mutating the input.
pub fn normalize_weekdays(days: &[u8]) -> Result<Vec<u8>, AlarmDraftError> {
    let unique: BTreeSet<u8> = days.iter().copied().collect();
    match unique.len() {
        1..=7 if unique.iter().all(|day| *day <= 6) => Ok(unique.into_iter().collect()),
        _ => Err(AlarmDraftError::WeekdaysInvalid),
    }
}

/// Bound, sort, and uniquify tags without mutating the input slice.
pub fn normalize_tags(tags: &[String]) -> Result<Vec<String>, AlarmDraftError> {
    let cleaned = tags
        .iter()
        .map(|tag| bounded_text(tag, MAX_TAG_CHARS, DraftField::Tag))
        .collect::<Result<Vec<_>, _>>()?;
    let unique: BTreeSet<String> = cleaned.into_iter().collect();
    match unique.len() {
        0..=MAX_TAGS => Ok(unique.into_iter().collect()),
        _ => Err(AlarmDraftError::TooManyTags),
    }
}

pub fn accept_volume(volume: f32) -> Result<f32, AlarmDraftError> {
    match volume {
        value if value.is_finite() && (0.0..=1.0).contains(&value) => Ok(value),
        _ => Err(AlarmDraftError::VolumeInvalid),
    }
}

pub fn accept_gradual_seconds(seconds: u32) -> Result<u32, AlarmDraftError> {
    match seconds {
        0..=MAX_GRADUAL_SECONDS => Ok(seconds),
        _ => Err(AlarmDraftError::GradualSecondsInvalid),
    }
}

fn decode_clock<'a>(
    original: &'a str,
    hour: &str,
    minute: &str,
    second: Option<&str>,
) -> Result<LocalTime<'a>, AlarmDraftError> {
    let hour = parse_two_digit_unit(hour, 23)?;
    let minute = parse_two_digit_unit(minute, 59)?;
    let second = match second {
        Some(second) => Some(parse_two_digit_unit(second, 59)?),
        None => None,
    };
    Ok(LocalTime {
        hour,
        minute,
        second,
        as_text: original,
    })
}

fn parse_two_digit_unit(part: &str, max: u8) -> Result<u8, AlarmDraftError> {
    match part.as_bytes() {
        [tens, ones] if tens.is_ascii_digit() && ones.is_ascii_digit() => {
            let value = (tens - b'0') * 10 + (ones - b'0');
            match value {
                value if value <= max => Ok(value),
                _ => Err(AlarmDraftError::LocalTimeInvalid),
            }
        }
        _ => Err(AlarmDraftError::LocalTimeInvalid),
    }
}

fn bounded_text(value: &str, max: usize, field: DraftField) -> Result<String, AlarmDraftError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(AlarmDraftError::EmptyOrControl { field });
    }
    if value.len() > max {
        return Err(AlarmDraftError::TooLong { field, max });
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft<'a>(
        local_time: &'a str,
        weekdays: &'a [u8],
        tags: &'a [String],
        volume: f32,
        gradual_seconds: u32,
    ) -> AlarmDraft<'a> {
        AlarmDraft {
            transition_id: "11111111-1111-4111-8111-111111111111",
            label: "Weekday",
            local_time,
            time_zone: "America/Chicago",
            weekdays,
            enabled: true,
            sound: "bell",
            volume,
            gradual_seconds,
            tags,
        }
    }

    #[test]
    fn weekdays_are_sorted_unique_and_bounded() {
        assert_eq!(normalize_weekdays(&[5, 1, 1, 3]).unwrap(), vec![1, 3, 5]);
        assert_eq!(normalize_weekdays(&[0]).unwrap(), vec![0]);
        assert_eq!(
            normalize_weekdays(&[]),
            Err(AlarmDraftError::WeekdaysInvalid)
        );
        assert_eq!(
            normalize_weekdays(&[1, 2, 3, 4, 5, 6, 0, 2]),
            Ok(vec![0, 1, 2, 3, 4, 5, 6])
        );
        assert_eq!(
            normalize_weekdays(&[7]),
            Err(AlarmDraftError::WeekdaysInvalid)
        );
        assert_eq!(
            normalize_weekdays(&[0, 1, 2, 3, 4, 5, 6, 1]),
            Ok(vec![0, 1, 2, 3, 4, 5, 6])
        );
    }

    #[test]
    fn local_time_parser_is_total_and_exhaustive() {
        let parsed = parse_local_time("07:30").unwrap();
        assert_eq!(
            parsed,
            LocalTime {
                hour: 7,
                minute: 30,
                second: None,
                as_text: "07:30",
            }
        );
        assert_eq!(parse_local_time("23:59:59").unwrap().second, Some(59));
        assert_eq!(
            parse_local_time("24:00"),
            Err(AlarmDraftError::LocalTimeInvalid)
        );
        assert_eq!(
            parse_local_time("7:30"),
            Err(AlarmDraftError::LocalTimeInvalid)
        );
        assert_eq!(
            parse_local_time("noon"),
            Err(AlarmDraftError::LocalTimeInvalid)
        );
        assert_eq!(
            parse_local_time("12:60"),
            Err(AlarmDraftError::LocalTimeInvalid)
        );
        assert_eq!(
            parse_local_time("12:30:60"),
            Err(AlarmDraftError::LocalTimeInvalid)
        );
        assert_eq!(
            parse_local_time("12:30:00:00"),
            Err(AlarmDraftError::LocalTimeInvalid)
        );
    }

    #[test]
    fn volume_and_gradual_seconds_reject_out_of_range() {
        assert_eq!(accept_volume(0.0), Ok(0.0));
        assert_eq!(accept_volume(1.0), Ok(1.0));
        assert_eq!(accept_volume(0.8), Ok(0.8));
        assert_eq!(accept_volume(-0.1), Err(AlarmDraftError::VolumeInvalid));
        assert_eq!(accept_volume(1.01), Err(AlarmDraftError::VolumeInvalid));
        assert_eq!(accept_volume(f32::NAN), Err(AlarmDraftError::VolumeInvalid));
        assert_eq!(accept_gradual_seconds(0), Ok(0));
        assert_eq!(accept_gradual_seconds(1800), Ok(1800));
        assert_eq!(
            accept_gradual_seconds(1801),
            Err(AlarmDraftError::GradualSecondsInvalid)
        );
    }

    #[test]
    fn tags_are_bounded_sorted_and_unique() {
        let tags = ["zeta".to_owned(), "alpha".to_owned(), "alpha".to_owned()];
        assert_eq!(
            normalize_tags(&tags).unwrap(),
            vec!["alpha".to_owned(), "zeta".to_owned()]
        );
        assert_eq!(
            normalize_tags(&["\u{0007}".to_owned()]),
            Err(AlarmDraftError::EmptyOrControl {
                field: DraftField::Tag
            })
        );
        let too_many: Vec<String> = (0..21).map(|index| format!("t{index:02}")).collect();
        assert_eq!(normalize_tags(&too_many), Err(AlarmDraftError::TooManyTags));
    }

    #[test]
    fn build_request_keeps_valid_draft_and_rejects_illegal_states() {
        let weekdays = [5, 1, 3];
        let tags = [String::from("work")];
        let request =
            build_create_alarm_request(draft("07:30", &weekdays, &tags, 0.8, 30)).unwrap();
        assert_eq!(request.weekdays, vec![1, 3, 5]);
        assert_eq!(request.local_time, "07:30");
        assert_eq!(request.volume, 0.8);
        assert_eq!(request.tags, vec!["work"]);

        assert_eq!(
            build_create_alarm_request(draft("25:00", &weekdays, &tags, 0.8, 30)),
            Err(AlarmDraftError::LocalTimeInvalid)
        );
        assert_eq!(
            build_create_alarm_request(draft("07:30", &[9], &tags, 0.8, 30)),
            Err(AlarmDraftError::WeekdaysInvalid)
        );
        assert_eq!(
            build_create_alarm_request(draft("07:30", &weekdays, &tags, 2.0, 30)),
            Err(AlarmDraftError::VolumeInvalid)
        );
    }
}
