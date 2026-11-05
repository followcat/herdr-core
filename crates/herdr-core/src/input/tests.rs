use super::*;

#[test]
fn quiet_release_remains_human_for_the_busy_gate_and_quiet_period() {
    use crate::viewers::InputKind;
    let release = b"\x1b[97;1:3u";
    for kind in [InputKind::Automatic, InputKind::QuietRelease] {
        let mut gate = Gate::default();
        gate.set_busy(true);
        for byte in release {
            assert!(gate.viewer(&[*byte], kind.human()).is_empty());
        }
        assert_eq!(gate.last_human.is_some(), kind == InputKind::QuietRelease);
        assert_eq!(gate.has_held(), kind == InputKind::QuietRelease);
        assert!(!gate.draft.pending, "a release does not type a draft");
        assert_eq!(gate.set_busy(false), release);
    }
}

#[test]
fn automatic_replies_preserve_the_human_quiet_gate() {
    let mut gate = Gate::default();
    assert_eq!(gate.viewer(b"reply", false), b"reply");
    assert!(gate.last_human.is_none());
    assert!(!gate.draft.pending);
    gate.viewer(b"x", true);
    assert!(gate.draft.pending);
    let human = gate.last_human;
    gate.set_busy(true);
    assert!(gate.viewer(b"automatic", false).is_empty());
    assert!(!gate.has_held(), "automatic replies are not a person interrupting a scroll");
    assert_eq!(gate.last_human, human);
    gate.viewer(b"\r\x15", false);
    assert!(gate.draft.pending, "automatic input cannot submit or clear a person's draft");
    gate.viewer(b"typed", true);
    assert!(gate.has_held());
    assert_eq!(gate.set_busy(false), b"automatic\r\x15typed");
    assert!(!gate.has_held());
}

#[test]
fn delivery_consumes_the_old_draft_but_preserves_held_human_keys() {
    for held in [b"".as_slice(), b"new draft", b"\x1b[97u", b"discarded\r"] {
        let mut gate = Gate::default();
        gate.viewer(b"old draft", true);
        gate.set_busy(true);
        gate.viewer(held, true);
        gate.viewer(b"automatic reply", false);
        gate.submitted();
        assert_eq!(gate.draft.pending, !held.is_empty() && !held.ends_with(b"\r"), "{held:?}");
        let mut expected = held.to_vec();
        expected.extend_from_slice(b"automatic reply");
        assert_eq!(gate.set_busy(false), expected);
    }
}

#[test]
fn drafts_clear_only_on_submit_or_unambiguous_clear_keys() {
    for clear in [
        b"\r".as_slice(),
        b"\x03",
        b"\x15",
        b"\x1b[13u",
        b"\x1b[57414u",
        b"\x1b[99;5u",
        b"\x1b[27;5;117~",
        b"\x1b[13;0;13;1;0;1_",
    ] {
        let mut draft = Draft::default();
        draft.feed(b"unfinished");
        for byte in clear {
            draft.feed(&[*byte]);
        }
        assert!(!draft.pending, "{clear:?}");
    }
    for keep in [b"\x7f".as_slice(), b"\x08", b"\x1b", b"\x1b[A", b"\x1b[13;1:3u"] {
        let mut draft = Draft::default();
        draft.feed(b"unfinished");
        draft.feed(keep);
        assert!(draft.pending, "{keep:?}");
    }
}

#[test]
fn paste_newlines_and_navigation_do_not_release_a_draft() {
    let mut draft = Draft::default();
    draft.feed(b"\x1b[200~\x1b[201~");
    assert!(!draft.pending, "an empty paste is not a draft");
    draft.feed(b"\x1b[A\x1b[<64;5;5M\x7f\x08");
    assert!(!draft.pending);
    for byte in b"\x1b[200~a\nb\r\x1b[201~" {
        draft.feed(&[*byte]);
    }
    assert!(draft.pending);
    draft.feed(b"\x1b\r");
    assert!(!draft.pending);
    for key in [b"\x1b[97u".as_slice(), b"\x1b[57400u", b"\x1b[65;30;97;1;0;1_", b"\x1b[27;1;97~"] {
        draft.feed(key);
        assert!(draft.pending, "{key:?}");
        draft.feed(b"\x15");
    }
}

#[test]
fn multiline_keys_do_not_release_drafts() {
    for key in [b"\n".as_slice(), b"\x1b[13;2u", b"\x1b[57414;2u", b"\x1b[27;2;13~"] {
        let mut draft = Draft::default();
        draft.feed(b"unfinished");
        draft.feed(key);
        assert!(draft.pending, "{key:?}");
    }
}

#[test]
fn word_navigation_and_escape_then_arrows_do_not_create_drafts() {
    for key in [
        b"\x1bb".as_slice(),
        b"\x1bf",
        b"\x1bB",
        b"\x1bF",
        b"\x1b\x1b[A",
        b"\x1b\x1bOD",
        b"\x1b[98;3u",
        b"\x1b[102;3u",
        b"\x1b[66;4u",
        b"\x1b[70;4u",
        b"\x1b[27;3;98~",
        b"\x1b[27;3;102~",
        b"\x1b[66;48;98;1;2;1_",
        b"\x1b[70;33;102;1;1;1_",
        b"\x1b[1;3D",
        b"\x1b[1;3C",
        b"\x1b[57350;3u",
        b"\x1b[57351;3u",
        b"\x1b[37;75;0;1;2;1_",
        b"\x1b[39;77;0;1;2;1_",
        b"\x01",
        b"\x05",
        b"\x1b[97;5u",
        b"\x1b[101;5u",
        b"\x1b[27;5;97~",
        b"\x1b[27;5;101~",
        b"\x1b[65;30;97;1;8;1_",
        b"\x1b[69;18;101;1;4;1_",
    ] {
        let mut draft = Draft::default();
        for byte in key {
            draft.feed(&[*byte]);
        }
        assert!(!draft.pending, "{key:?}");
        draft.feed(b"unfinished");
        draft.feed(key);
        assert!(draft.pending, "navigation must preserve an existing draft: {key:?}");
    }
}

#[test]
fn control_string_replies_do_not_create_or_clear_drafts() {
    for reply in [
        b"\x1b_Gi=31;OK\x1b\\".as_slice(),
        b"\x1b]11;rgb:0000/0000/0000\x1b\\",
        b"\x1b]11;rgb:0000/0000/0000\x07",
        b"\x1bP>|terminal-name\x1b\\",
        b"\x1bP1+r544e=787465726d\x1b\\",
        b"\x1bP!|1234\x1b\\",
        b"\x1bP+r544e=787465726d\x1b\\",
        b"\x1bP$r0m\x1b\\",
    ] {
        for split in 0..=reply.len() {
            let mut draft = Draft::default();
            draft.feed(&reply[..split]);
            draft.feed(&reply[split..]);
            assert!(!draft.pending, "reply: {reply:?}, split {split}");
        }
        let mut draft = Draft::default();
        for byte in reply {
            draft.feed(&[*byte]);
        }
        assert!(!draft.pending, "bytewise reply: {reply:?}");
        draft.feed(b"x");
        assert!(draft.pending, "tracking resumes after reply: {reply:?}");
        for byte in reply {
            draft.feed(&[*byte]);
            assert!(draft.pending, "reply must preserve typing: {reply:?}");
        }
        draft.feed(b"\r");
        assert!(!draft.pending);
    }
}

#[test]
fn long_control_string_replies_preserve_the_previous_draft() {
    for reply in [format!("\x1b]52;c;{}\x07", "YWFh".repeat(256)), format!("\x1bP1+r544e={}\x1b\\", "78".repeat(512))] {
        for pending in [false, true] {
            for chunk_size in [1, 31, reply.len()] {
                let mut draft = Draft { pending, ..Default::default() };
                for chunk in reply.as_bytes().chunks(chunk_size) {
                    draft.feed(chunk);
                }
                assert_eq!(draft.pending, pending, "long reply with chunks of {chunk_size}");
                draft.feed(b"\r");
                assert!(!draft.pending);
                draft.feed(b"y");
                assert!(draft.pending);
            }
        }
    }
}

#[test]
fn alt_control_string_introducers_with_ordinary_text_are_drafts() {
    for key in [b"\x1b]hello".as_slice(), b"\x1b_world", b"\x1bPlease", b"\x1b]\r"] {
        let mut draft = Draft::default();
        for byte in key {
            draft.feed(&[*byte]);
        }
        assert_eq!(draft.pending, !key.ends_with(b"\r"), "{key:?}");
    }
}

#[test]
fn standalone_alt_introducers_protect_drafts_until_a_reply_completes() {
    for key in [b"\x1b]".as_slice(), b"\x1b_", b"\x1bP"] {
        let mut draft = Draft::default();
        draft.feed(key);
        assert!(draft.pending, "standalone Alt key: {key:?}");
    }
}

#[test]
fn invalid_control_strings_return_to_typing_and_clear_keys() {
    for prefix in [b"\x1b]1hello".as_slice(), b"\x1b_Ghello", b"\x1bP>hello"] {
        for invalid in [b'\r', b'\n', 3, 21, 127] {
            let mut draft = Draft::default();
            draft.feed(b"old draft");
            for byte in prefix {
                draft.feed(&[*byte]);
            }
            draft.feed(&[invalid]);
            assert_eq!(draft.pending, matches!(invalid, b'\n' | 127), "{prefix:?}, {invalid}");
            draft.feed(b"new draft");
            assert!(draft.pending);
        }
        let mut draft = Draft::default();
        draft.feed(prefix);
        draft.feed(b"\x1b");
        draft.feed(b"[97u");
        assert!(draft.pending, "a new escape sequence aborts the candidate: {prefix:?}");
        draft.feed(b"\r");
        assert!(!draft.pending);
    }
    let mut draft = Draft::default();
    draft.feed(b"\x1b]1h");
    assert!(draft.pending, "OSC selectors require a semicolon, not ordinary text");
    draft.feed(b"\x07");
    assert!(draft.pending, "an invalid OSC cannot later be completed as a reply");
}

#[test]
fn other_alt_keys_still_create_drafts() {
    for key in [b"\x1bx".as_slice(), b"\x1b[120;3u", b"\x1b[27;3;120~", b"\x1b[88;45;120;1;2;1_"] {
        let mut draft = Draft::default();
        draft.feed(key);
        assert!(draft.pending, "{key:?}");
    }
}

#[test]
fn paste_ending_in_escape_still_recognizes_the_end_marker() {
    for clear in [b'\r', 3, 21] {
        let mut draft = Draft::default();
        for byte in b"\x1b[200~abc\x1b\x1b[201~" {
            draft.feed(&[*byte]);
        }
        assert!(draft.pending);
        assert!(!draft.pasted);
        draft.feed(&[clear]);
        assert!(!draft.pending);
    }
}

#[test]
fn pastes_are_bracketed_only_when_asked_and_cannot_end_early() {
    assert_eq!(paste(true, "hi"), b"\x1b[200~hi\x1b[201~");
    assert_eq!(paste(false, "hi"), b"hi");
    assert_eq!(paste(true, "a\x1b[201~b"), b"\x1b[200~ab\x1b[201~");
}
