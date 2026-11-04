// SPDX-License-Identifier: AGPL-3.0-only
use super::*;
use crate::crypto::{Protector, Vault};

struct Plain;
impl Protector for Plain {
    fn protect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
        Ok(d.to_vec())
    }
    fn unprotect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
        Ok(d.to_vec())
    }
}

fn setup() -> (PathBuf, Connection, Cipher) {
    let d = std::env::temp_dir().join(format!("sulcusai-mt-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    let conn = db::open(&d.join("t.db")).unwrap();
    let c = Vault::open(&d.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap();
    (d, conn, c)
}

#[test]
fn batched_phrases_map_back_to_meeting_time() {
    let u = |start_secs: f64, secs: f64| vad::Utterance {
        start: (start_secs * 16_000.0) as u64,
        samples: vec![0.1; (secs * 16_000.0) as usize],
    };
    let mut b = Batch::default();
    b.add(u(10.0, 2.0));
    b.add(u(30.0, 3.0));
    assert!((b.secs() - 5.5).abs() < 1e-9, "two phrases and one short gap");
    let job = b.take(Speaker::Others).unwrap();
    assert!(b.take(Speaker::Others).is_none());
    assert!((job.real_time(0.5) - 10.5).abs() < 1e-9);
    // In the gap or past a piece's end, times stick to that piece.
    assert!((job.real_time(2.2) - 12.0).abs() < 1e-9);
    assert!((job.real_time(3.0) - 30.5).abs() < 1e-9);
}

#[test]
fn broken_lines_are_rejoined_into_sentences() {
    let l = |a: f64, b: f64, t: &str| (a, b, t.to_string());
    let out = whole_sentences(vec![
        l(8.9, 11.9, "Jordan, please send the budget by"),
        l(11.9, 12.5, " Friday."),
        l(15.0, 17.0, "Sounds good, I'll book the"),
        l(19.5, 20.0, "venue."),
    ]);
    assert_eq!(out.len(), 3, "a long pause keeps lines apart");
    assert_eq!(out[0].2, "Jordan, please send the budget by Friday.");
    assert_eq!(out[0].1, 12.5);
}

#[test]
fn echo_of_the_call_is_recognized() {
    assert!(is_echo("we should ship on Thursday", "We should ship it on Thursday."));
    assert!(!is_echo("I disagree, Friday is better", "We should ship it on Thursday."));
    assert!(!is_echo("yes", "yes"), "single words are too short to judge");
}

#[test]
fn meetings_and_segments_are_stored_encrypted() {
    let (_d, conn, c) = setup();
    let m = create(&conn, &c, &MeetingData { title: "Budget sync".into(), ..Default::default() }).unwrap();
    add_segment(&conn, &c, &m.id, Speaker::Others, None, 1.0, 3.0, "The budget is approved.").unwrap();
    add_segment(&conn, &c, &m.id, Speaker::You, None, 0.2, 0.9, "Hi all.").unwrap();
    let raw: String = conn.query_row("SELECT text FROM meeting_segments LIMIT 1", [], |r| r.get(0)).unwrap();
    assert!(raw.starts_with("enc1:"));
    let raw: String = conn.query_row("SELECT data FROM meetings", [], |r| r.get(0)).unwrap();
    assert!(!raw.contains("Budget"));
    let segs = segments(&conn, &c, &m.id);
    assert_eq!(segs[0].text, "Hi all.", "ordered by time");
    assert_eq!(transcript_text(&segs, &BTreeMap::new()), "[00:00] You: Hi all.\n[00:01] Others: The budget is approved.");
    // Speaker labels and names.
    add_segment(&conn, &c, &m.id, Speaker::Others, Some(2), 5.0, 6.0, "Agreed.").unwrap();
    let segs = segments(&conn, &c, &m.id);
    let names = BTreeMap::from([(2u32, "Jordan".to_string())]);
    assert!(transcript_text(&segs, &BTreeMap::new()).ends_with("Speaker 2: Agreed."));
    assert!(transcript_text(&segs, &names).ends_with("Jordan: Agreed."));
    let hits = search(&conn, &c, "budget approved", 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].1.len(), 1);
    // Deleting the meeting deletes its transcript.
    conn.execute("DELETE FROM meetings WHERE id = ?1", [&m.id]).unwrap();
    assert!(segments(&conn, &c, &m.id).is_empty());
}

#[test]
fn audio_chunks_round_trip_across_a_boundary() {
    let (d, _conn, c) = setup();
    let mut w = AudioWriter::new(&d, Speaker::You, c.clone());
    // 40 s of a ramp, written in uneven pieces.
    let total = 40 * audio::RATE as usize;
    let signal: Vec<f32> = (0..total).map(|i| ((i % 1000) as f32 / 1000.0) - 0.5).collect();
    for piece in signal.chunks(7_777) {
        w.push(piece);
    }
    w.finish();
    let got = read_channel(&d, Speaker::You, &c, 29.0, 31.0);
    assert_eq!(got.len(), 2 * audio::RATE as usize);
    let want = &signal[29 * 16_000..31 * 16_000];
    assert!(got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3));
    // The stored files aren't readable audio.
    let raw = std::fs::read(d.join("you-00000.enc")).unwrap();
    assert!(raw.len() > CHUNK * 2);
    // Mixed clip of a channel that wasn't recorded is just the other one.
    let wav = clip(&d, &c, 0.0, 1.0);
    assert_eq!(audio::wav_decode(&wav).unwrap().samples.len(), audio::RATE as usize);
}

#[test]
fn notes_are_parsed_from_a_wordy_reply() {
    let reply = "Here you go:\n```json\n{\"title\":\"Launch\",\"summary\":\"s\",\"topics\":[],\"key_points\":[],\"most_important\":[\"ship\"],\"decisions\":[],\"action_items\":[{\"task\":\"Send budget\",\"owner\":\"Jordan\",\"due\":\"Friday\"}]}\n```";
    let n = parse_notes(reply).unwrap();
    assert_eq!(n.action_items[0].owner, "Jordan");
    assert!(!n.action_items[0].done);
}

#[test]
fn markdown_export_has_notes_and_transcript() {
    let m = Meeting {
        id: "m".into(),
        started_at: 0,
        ended_at: None,
        status: "done".into(),
        data: MeetingData {
            title: "Launch".into(),
            notes: Some(Notes {
                summary: "We set a date.".into(),
                action_items: vec![ActionItem { task: "Send budget".into(), owner: "Jordan".into(), due: "Friday".into(), done: true }],
                ..Default::default()
            }),
            ..Default::default()
        },
    };
    let segs = vec![Segment { id: 1, speaker: Speaker::Others, voice: None, start: 65.0, end: 70.0, text: "Thursday works.".into(), translation: None }];
    let md = notes_markdown(&m, Some(&segs));
    assert!(md.starts_with("# Launch"));
    assert!(md.contains("- [x] Send budget (Jordan, due Friday)"));
    assert!(md.contains("**[01:05] Others:** Thursday works."));
}

#[test]
fn typed_notes_come_before_the_transcript() {
    assert_eq!(with_typed("[00:01] You: hi", "  "), "[00:01] You: hi");
    let both = with_typed("[00:01] You: hi", "Jordan owns the budget");
    assert!(both.starts_with("Notes the user typed") && both.contains("Jordan owns the budget") && both.ends_with("What was said:\n[00:01] You: hi"));
}
