//! EMOJI (EMOJI-plan 3.4, 4 V1): candidate strings with emoji (a surrogate pair, a variation selector) and the
//! committed emoji text survive the wire in every protocol version; the format itself is unchanged (golden tests).
use twime_proto::{Cands, Reply, Update};

#[test]
fn emoji_strings_round_trip_in_all_versions() {
    let items: Vec<String> = ["愛心", "\u{1F600}", "\u{2764}\u{FE0F}", "\u{1F441}\u{FE0F}", "6\u{FE0F}\u{20E3}"].iter().map(|s| s.to_string()).collect();
    let u = Update {
        handled: true,
        commit: Some("\u{2764}\u{FE0F}，".into()),
        candidates: Some(Cands { items: items.clone(), selected: 4, page: 0, total: 5, per_page: 9 }),
        candidates_open: true,
        ..Update::default()
    };
    for v in 1..=twime_proto::PROTOCOL_VERSION {
        let bytes = Reply::Update { session: 7, seq: 9, update: u.clone() }.encode_for(v);
        let Some(Reply::Update { update, .. }) = Reply::decode(&bytes) else { panic!("v{v}: no update") };
        let c = update.candidates.expect("candidates");
        assert_eq!(c.items, items, "v{v}");
        assert_eq!(c.selected, 4, "v{v}");
        assert_eq!(update.commit.as_deref(), Some("\u{2764}\u{FE0F}，"), "v{v}");
    }
}
