use herdr_white_noise_bridge::activity::{Activity, UserInput};

fn input(id: &str, text: &str) -> UserInput {
    UserInput::new(id.into(), text)
}

#[test]
fn first_history_is_a_baseline_and_new_ids_are_local_even_with_old_text() {
    let mut state = Activity::default();
    assert!(!state.observe(&[input("old", "hello")]));
    assert!(state.observe(&[input("old", "hello"), input("new", "hello")]));
    assert!(!state.observe(&[input("old", "hello"), input("new", "hello")]));
}

#[test]
fn only_one_confirmed_unambiguous_phone_echo_is_exempted() {
    let mut state = Activity::default();
    state.start_phone("receipt", "hello\r\nworld", &[]);
    state.confirm_phone("receipt");
    let serialized = serde_json::to_string(&state).unwrap();
    assert!(!serialized.contains("hello"));
    let mut state: Activity = serde_json::from_str(&serialized).unwrap();
    assert!(!state.observe(&[input("phone", "hello\nworld")]));
    assert!(state.observe(&[
        input("phone", "hello\nworld"),
        input("local", "hello\nworld")
    ]));
}

#[test]
fn uncertain_cancelled_and_duplicate_submissions_cannot_hide_local_activity() {
    for mode in ["uncertain", "cancelled", "duplicate"] {
        let mut state = Activity::default();
        state.start_phone("receipt", "hello", &[]);
        match mode {
            "cancelled" => state.cancel_phone("receipt"),
            "duplicate" => {
                state.confirm_phone("receipt");
                state.start_phone("other", "hello", &[]);
                state.confirm_phone("other");
            }
            _ => {}
        }
        assert!(state.observe(&[input("local", "hello")]), "{mode}");
    }
}

#[test]
fn simultaneous_same_text_messages_and_media_without_text_mute() {
    let mut state = Activity::default();
    state.start_phone("receipt", "hello", &[]);
    state.confirm_phone("receipt");
    assert!(state.observe(&[input("phone", "hello"), input("local", "hello")]));
    assert!(state.observe(&[input("image", "")]));
}

#[test]
fn historical_same_text_does_not_make_a_phone_echo_ambiguous() {
    let mut state = Activity::default();
    state.start_phone("receipt", "hello", &[input("old", "hello")]);
    state.confirm_phone("receipt");
    assert!(!state.observe(&[input("old", "hello"), input("phone", "hello")]));
}

#[test]
fn incomplete_phone_echo_waits_for_text_without_consuming_attribution() {
    let mut state = Activity::default();
    state.start_phone("receipt", "hello", &[]);
    state.confirm_phone("receipt");
    assert!(!state.observe(&[UserInput::pending("phone".into())]));
    assert!(state.unresolved());
    let mut state: Activity =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    assert!(!state.observe(&[input("phone", "hello")]));
    assert!(!state.unresolved());
}

#[test]
fn a_phone_echo_records_the_turn_that_answers_it() {
    let mut state = Activity::default();
    state.baseline(&[]);
    state.start_phone("receipt", "hello", &[]);
    state.confirm_phone("receipt");
    assert!(!state.observe(&[input("phone", "hello").in_turn(Some("turn-1".into()))]));
    assert!(state.phone_turn("turn-1"));
    assert!(state.observe(&[
        input("phone", "hello").in_turn(Some("turn-1".into())),
        input("peer", "hello").in_turn(Some("turn-2".into())),
    ]));
    assert!(!state.phone_turn("turn-2"));
    state.retain_turns(&["turn-2".into()]);
    assert!(!state.phone_turn("turn-1"));
}

#[test]
fn queued_input_never_mutes_and_a_queued_phone_echo_marks_the_running_turn() {
    let mut state = Activity::default();
    state.baseline(&[input("peer", "work").in_turn(Some("turn-1".into()))]);
    state.start_phone("receipt", "hello", &[]);
    state.confirm_phone("receipt");
    let peer = input("peer", "work").in_turn(Some("turn-1".into()));
    let other = input("other", "more work")
        .in_turn(Some("turn-1".into()))
        .queued();
    assert!(!state.observe(&[peer.clone(), other.clone()]));
    assert!(!state.phone_turn("turn-1"));
    let phone = input("phone", "hello")
        .in_turn(Some("turn-1".into()))
        .queued();
    assert!(!state.observe(&[peer, other, phone]));
    assert!(state.phone_turn("turn-1"));
}
