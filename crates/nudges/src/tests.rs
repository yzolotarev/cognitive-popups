use super::*;

const T0: i64 = 1_800_000_000;
const MIDNIGHT: i64 = T0 - 9 * HOUR;

fn with_consent(kinds: &[Consent]) -> Nudges {
    let mut n = Nudges::open_in_memory().unwrap();
    for kind in kinds {
        n.set_consent(*kind, true, T0).unwrap();
    }
    n
}

fn busy() -> Context {
    Context {
        slack: Slack::Busy,
        media: false,
    }
}

#[test]
fn inv09_without_consent_nothing_is_shown_recorded_or_randomized() {
    let mut n = Nudges::open_in_memory().unwrap();
    assert_eq!(
        n.check(Kind::Opportunity, "t", T0, MIDNIGHT, Context::FREE)
            .unwrap(),
        Some(Silence::NoConsent)
    );
    assert_eq!(
        n.check(Kind::Daily, "t", T0, MIDNIGHT, Context::FREE)
            .unwrap(),
        Some(Silence::NoConsent)
    );
    assert!(!n.may_record().unwrap());
    let decision = n
        .decide("e1", "u1", T0, Context::FREE, 0.0, 0.5, false)
        .unwrap();
    assert!(matches!(
        decision,
        Decision::NotEligible {
            silence: Silence::NoConsent,
            ..
        }
    ));
    n.set_consent(Consent::Opportunity, true, T0).unwrap();
    n.set_consent(Consent::Opportunity, false, T0 + 1).unwrap();
    assert_eq!(
        n.check(Kind::Opportunity, "t", T0, MIDNIGHT, Context::FREE)
            .unwrap(),
        Some(Silence::NoConsent),
        "a withdrawn consent is a «no»"
    );
}

#[test]
fn a_busy_reader_or_a_video_gets_nothing() {
    let n = with_consent(&[Consent::Opportunity]);
    assert_eq!(
        n.check(Kind::Opportunity, "t", T0, MIDNIGHT, busy())
            .unwrap(),
        Some(Silence::Busy)
    );
    let video = Context {
        slack: Slack::Free,
        media: true,
    };
    assert_eq!(
        n.check(Kind::Opportunity, "t", T0, MIDNIGHT, video)
            .unwrap(),
        Some(Silence::Media)
    );
}

#[test]
fn one_budget_three_a_day_two_hours_apart() {
    let mut n = with_consent(&[Consent::Opportunity, Consent::Daily]);
    let free = Context::FREE;
    assert_eq!(
        n.check(Kind::Daily, "plan", T0, MIDNIGHT, free).unwrap(),
        None
    );
    n.record_shown(Kind::Daily, "plan", T0).unwrap();
    assert_eq!(
        n.check(Kind::Opportunity, "a", T0 + HOUR, MIDNIGHT, free)
            .unwrap(),
        Some(Silence::TooSoon {
            next_at: T0 + MIN_GAP
        }),
        "Daily and Opportunity share the gap"
    );
    n.record_shown(Kind::Opportunity, "a", T0 + 2 * HOUR)
        .unwrap();
    n.record_shown(Kind::Opportunity, "b", T0 + 4 * HOUR)
        .unwrap();
    assert_eq!(
        n.check(Kind::Opportunity, "c", T0 + 7 * HOUR, MIDNIGHT, free)
            .unwrap(),
        Some(Silence::DailyLimit)
    );
    assert_eq!(
        n.check(Kind::Daily, "plan", T0 + 7 * HOUR, MIDNIGHT, free)
            .unwrap(),
        Some(Silence::AlreadyToday)
    );
    let tomorrow = MIDNIGHT + DAY;
    assert_eq!(
        n.check(Kind::Opportunity, "c", tomorrow + 9 * HOUR, tomorrow, free)
            .unwrap(),
        None
    );
}

#[test]
fn not_now_silences_everything_for_a_day_and_a_topic_does_not_repeat() {
    let mut n = with_consent(&[Consent::Opportunity, Consent::Daily]);
    let show = n
        .record_shown(Kind::Opportunity, "вернись к гипотезе", T0)
        .unwrap();
    assert!(n.record_response(show, Response::NotNow, T0 + 60).unwrap());
    assert!(
        !n.record_response(show, Response::Opened, T0 + 90).unwrap(),
        "first answer counts"
    );
    let next_day = MIDNIGHT + DAY;
    assert_eq!(
        n.check(Kind::Daily, "plan", T0 + 5 * HOUR, MIDNIGHT, Context::FREE)
            .unwrap(),
        Some(Silence::Snoozed {
            until: T0 + 60 + SNOOZE
        })
    );
    let after = T0 + 60 + SNOOZE;
    assert_eq!(
        n.check(
            Kind::Opportunity,
            "вернись к гипотезе",
            after - 1,
            next_day,
            Context::FREE
        )
        .unwrap(),
        Some(Silence::Snoozed { until: after })
    );
    let mut n = with_consent(&[Consent::Opportunity]);
    n.record_shown(Kind::Opportunity, "x", T0).unwrap();
    assert!(matches!(
        n.check(
            Kind::Opportunity,
            "x",
            T0 + 3 * HOUR,
            MIDNIGHT,
            Context::FREE
        )
        .unwrap(),
        Some(Silence::RepeatTopic { .. })
    ));
    assert_eq!(
        n.check(
            Kind::Opportunity,
            "y",
            T0 + 3 * HOUR,
            MIDNIGHT,
            Context::FREE
        )
        .unwrap(),
        None
    );
}

#[test]
fn mrt_randomizes_only_meaningful_moments_and_once_per_point() {
    let mut n = with_consent(&[Consent::Experiments]);
    assert!(matches!(
        n.decide("e1", "u1", T0, Context::FREE, 0.1, 0.5, false)
            .unwrap(),
        Decision::Randomized {
            arm: Arm::Diagnostic,
            ..
        }
    ));
    assert!(matches!(
        n.decide("e2", "u1", T0, Context::FREE, 0.9, 0.5, false)
            .unwrap(),
        Decision::Randomized {
            arm: Arm::Silence,
            ..
        }
    ));
    assert_eq!(
        n.decide("e1", "u1", T0, Context::FREE, 0.9, 0.5, false)
            .unwrap(),
        Decision::Duplicate
    );
    assert!(matches!(
        n.decide("e3", "u1", T0, busy(), 0.1, 0.5, false).unwrap(),
        Decision::NotEligible {
            silence: Silence::Busy,
            ..
        }
    ));
    let summary = n.summary().unwrap();
    assert_eq!(
        summary.iter().map(|s| s.decisions).sum::<usize>(),
        2,
        "busy is outside the trial"
    );
}

#[test]
fn outcome_is_a_check_one_to_seven_days_later_else_missing() {
    let mut n = with_consent(&[Consent::Experiments]);
    n.decide("e1", "u1", T0, Context::FREE, 0.1, 0.5, false)
        .unwrap();
    n.decide("e2", "u2", T0, Context::FREE, 0.9, 0.5, false)
        .unwrap();
    assert_eq!(
        n.record_check("u1", true, T0 + HOUR).unwrap(),
        0,
        "too early: same session"
    );
    assert_eq!(n.record_check("u1", true, T0 + 3 * DAY).unwrap(), 1);
    assert_eq!(
        n.record_check("u1", false, T0 + 4 * DAY).unwrap(),
        0,
        "the first check counts"
    );
    assert_eq!(n.close_missing(T0 + 6 * DAY).unwrap(), 0);
    assert_eq!(n.close_missing(T0 + 8 * DAY).unwrap(), 1);
    let summary = n.summary().unwrap();
    let diag = summary.iter().find(|s| s.arm == Arm::Diagnostic).unwrap();
    let silence = summary.iter().find(|s| s.arm == Arm::Silence).unwrap();
    assert_eq!((diag.correct, diag.missing), (1, 0));
    assert_eq!((silence.correct, silence.missing), (0, 1));
}

#[test]
fn engagement_is_kept_apart_from_the_verified_result() {
    let mut n = with_consent(&[Consent::Experiments]);
    let Decision::Randomized { id, .. } = n
        .decide("e1", "u1", T0, Context::FREE, 0.1, 0.5, false)
        .unwrap()
    else {
        panic!()
    };
    n.record_engaged(id, true).unwrap();
    let diag = &n.summary().unwrap()[0];
    assert_eq!((diag.engaged, diag.correct), (1, 0));
}

/// A/A: both labels are the same silence; with outcomes independent of the
/// label the arms must come out alike (P8 acceptance).
#[test]
fn aa_check_shows_no_difference_between_identical_arms() {
    let mut n = with_consent(&[Consent::Experiments]);
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut uniform = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    for i in 0..2000 {
        let unit = format!("u{i}");
        n.decide(
            &format!("e{i}"),
            &unit,
            T0,
            Context::FREE,
            uniform(),
            0.5,
            true,
        )
        .unwrap();
        n.record_check(&unit, uniform() < 0.6, T0 + 2 * DAY)
            .unwrap();
    }
    let summary = n.summary().unwrap();
    let rate = |s: &ArmSummary| s.correct as f64 / (s.correct + s.incorrect) as f64;
    let (a, b) = (&summary[0], &summary[1]);
    assert_eq!((a.arm, b.arm), (Arm::AaFirst, Arm::AaSecond));
    let p = (a.correct + b.correct) as f64 / (a.decisions + b.decisions) as f64;
    let se = (p * (1.0 - p) * (1.0 / a.decisions as f64 + 1.0 / b.decisions as f64)).sqrt();
    let z = (rate(a) - rate(b)) / se;
    assert!(z.abs() < 1.96, "A/A difference z = {z:.2}");
    assert!((a.decisions as f64 - 1000.0).abs() < 100.0, "balanced arms");
}

#[test]
fn card_texts_inform_rather_than_order() {
    for kind in [Kind::Opportunity, Kind::Daily] {
        let text = card_text(kind);
        assert!(text.starts_with("Есть"), "{text}");
        assert!(!text.contains('!'));
    }
}

#[test]
fn the_review_pill_is_asked_once_and_off_by_default() {
    let mut nudges = Nudges::open_in_memory().unwrap();
    assert!(!nudges.consented(Consent::Reviews).unwrap());
    assert!(!nudges.consent_asked(Consent::Reviews).unwrap());
    nudges.set_consent(Consent::Reviews, false, 1).unwrap();
    assert!(nudges.consent_asked(Consent::Reviews).unwrap());
    assert!(!nudges.consented(Consent::Reviews).unwrap());
}
