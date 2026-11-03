// SPDX-License-Identifier: AGPL-3.0-only
use super::*;

const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:abc@example.com\r\nSUMMARY:Budget review\\, Q4\r\nDTSTART;TZID=America/Denver:20261009T140000\r\nDTEND;TZID=America/Denver:20261009T150000\r\nLOCATION:Room 2\r\nDESCRIPTION:Bring the\\n numbers\r\nATTENDEE;CN=Ana:mailto:ana@example.com\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:day@x\r\nSUMMARY:Holiday\r\nDTSTART;VALUE=DATE:20261012\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:gone@x\r\nSUMMARY:Cancelled thing\r\nSTATUS:CANCELLED\r\nDTSTART:20261009T180000Z\r\nDURATION:PT30M\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:long@x\r\nSUMMARY:A very long title that the ser\r\n ver folded\r\nDTSTART:20261010T170000Z\r\nDURATION:PT1H30M\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[test]
fn new_events_are_named_after_their_uid() {
    let cal = "https://apidata.googleusercontent.com/caldav/v2/me%40gmail.com/events/";
    assert_eq!(event_url(cal, "4e11-bf37@sulcusai"), format!("{cal}4e11-bf37%40sulcusai.ics"));
    assert_eq!(event_url("https://dav.example.com/cal", "a b"), "https://dav.example.com/cal/a%20b.ics");
}

#[test]
fn outlook_events_are_read() {
    let v = json!({
        "id": "AAMk1", "subject": "Standup", "isAllDay": false, "isCancelled": false, "type": "occurrence", "changeKey": "ck",
        "start": { "dateTime": "2026-10-09T14:00:00.0000000", "timeZone": "UTC" },
        "end": { "dateTime": "2026-10-09T14:30:00.0000000", "timeZone": "UTC" },
        "location": { "displayName": "Teams" }, "bodyPreview": "Daily",
        "attendees": [{ "emailAddress": { "address": "ana@example.com" } }]
    });
    let (s, e, all_day, d) = graph_event(&v).unwrap();
    assert_eq!(s, chrono::DateTime::parse_from_rfc3339("2026-10-09T14:00:00Z").unwrap().timestamp_millis());
    assert_eq!(e - s, 30 * 60_000);
    assert!(!all_day && d.recurring);
    assert_eq!((d.title.as_str(), d.location.as_str(), d.attendees.len()), ("Standup", "Teams", 1));
    assert_eq!(d.href.as_deref(), Some("https://graph.microsoft.com/v1.0/me/events/AAMk1"));
    assert!(graph_event(&json!({ "isCancelled": true, "start": v["start"] })).is_none());
    let holiday = json!({ "id": "x", "subject": "Holiday", "isAllDay": true, "start": { "dateTime": "2026-10-12T00:00:00.0000000" }, "end": { "dateTime": "2026-10-13T00:00:00.0000000" } });
    let (s, e, all_day, _) = graph_event(&holiday).unwrap();
    assert!(all_day);
    assert_eq!(e - s, DAY_MS);
    // Local midnight on that date, whatever this PC's zone.
    let local = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(s).unwrap().with_timezone(&chrono::Local);
    assert_eq!(local.format("%Y-%m-%d %H:%M").to_string(), "2026-10-12 00:00");
    // A time in this PC's zone (as asked with Prefer) is read as local time.
    let local_start = json!({ "dateTime": "2026-10-09T14:00:00.0000000", "timeZone": "Mountain Standard Time" });
    let ms = graph_ms(&local_start, false).unwrap();
    let back = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms).unwrap().with_timezone(&chrono::Local);
    assert_eq!(back.format("%H:%M").to_string(), "14:00");
}

#[cfg(windows)]
#[test]
fn windows_names_this_pcs_time_zone() {
    let tz = windows_time_zone().unwrap();
    println!("time zone: {tz}");
    assert!(tz.contains("Time") || tz == "UTC", "{tz}");
}

#[test]
fn icalendar_is_read() {
    let ev = parse_ics(ICS);
    assert_eq!(ev.len(), 3, "the cancelled one is skipped");
    let (s, e, all_day, d) = &ev[0];
    assert_eq!(d.title, "Budget review, Q4");
    assert_eq!(d.notes, "Bring the\n numbers");
    assert_eq!(d.attendees, vec!["ana@example.com"]);
    // 2pm in Denver (MDT, UTC-6) is 20:00 UTC.
    assert_eq!(*s, Utc.with_ymd_and_hms(2026, 10, 9, 20, 0, 0).unwrap().timestamp_millis());
    assert_eq!(e - s, 3_600_000);
    assert!(!all_day);
    assert!(ev[1].2 && ev[1].1 - ev[1].0 == DAY_MS, "all-day, one day long");
    assert_eq!(ev[2].3.title, "A very long title that the server folded");
    assert_eq!(ev[2].1 - ev[2].0, 90 * 60_000);
}

#[test]
fn events_round_trip_through_ics() {
    let e = Event {
        id: "x".into(),
        account_id: None,
        calendar: None,
        start: Utc.with_ymd_and_hms(2026, 10, 9, 20, 0, 0).unwrap().timestamp_millis(),
        end: Utc.with_ymd_and_hms(2026, 10, 9, 21, 0, 0).unwrap().timestamp_millis(),
        all_day: false,
        data: EventData { title: "Lunch; with Bo, maybe".into(), notes: "line1\nline2".into(), attendees: vec!["bo@x.com".into()], uid: "u1@sulcusai".into(), ..Default::default() },
    };
    let ics = to_ics(&e, Some("me@x.com"));
    let back = parse_ics(&ics);
    assert_eq!(back.len(), 1);
    assert_eq!((back[0].0, back[0].1), (e.start, e.end));
    assert_eq!(back[0].3.title, e.data.title);
    assert_eq!(back[0].3.notes, e.data.notes);
    assert_eq!(back[0].3.attendees, vec!["bo@x.com"]);
    assert!(ics.contains("ORGANIZER:mailto:me@x.com"));
}

const INVITE: &str ="BEGIN:VCALENDAR\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\nVERSION:2.0\r\nCALSCALE:GREGORIAN\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nDTSTART:20261023T210000Z\r\nDTEND:20261023T220000Z\r\nDTSTAMP:20261019T150000Z\r\nORGANIZER;CN=\"Ruiz, Ana\":mailto:ana@example.com\r\nUID:7kq2c4ab@google.com\r\nATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;CN=\"Ruiz, Ana\";X-NUM-GUESTS=0:mailto:ana@example.com\r\nATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;CN=Isaac Lehman:mailto:isaac@example.org\r\nSEQUENCE:2\r\nSUMMARY:Budget review\r\nLOCATION:Room 2\r\nDESCRIPTION:Bring the numbers\r\nSTATUS:CONFIRMED\r\nBEGIN:VALARM\r\nACTION:EMAIL\r\nATTENDEE:mailto:alarm@example.com\r\nTRIGGER:-P0DT0H30M0S\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[test]
fn invitations_are_read() {
    let inv = parse_invite(INVITE).unwrap();
    assert_eq!((inv.method.as_str(), inv.uid.as_str(), inv.sequence), ("REQUEST", "7kq2c4ab@google.com", 2));
    assert_eq!((inv.title.as_str(), inv.location.as_str(), inv.notes.as_str()), ("Budget review", "Room 2", "Bring the numbers"));
    assert_eq!(inv.start, Utc.with_ymd_and_hms(2026, 10, 23, 21, 0, 0).unwrap().timestamp_millis());
    assert_eq!(inv.end - inv.start, 3_600_000);
    assert_eq!((inv.organizer.as_str(), inv.organizer_name.as_str()), ("ana@example.com", "Ruiz, Ana"));
    let people: Vec<(&str, &str, &str)> = inv.attendees.iter().map(|a| (a.email.as_str(), a.name.as_str(), a.partstat.as_str())).collect();
    assert_eq!(people, vec![("ana@example.com", "Ruiz, Ana", "ACCEPTED"), ("isaac@example.org", "Isaac Lehman", "NEEDS-ACTION")], "the alarm's address isn't a guest");
    assert!(!inv.recurring && inv.recurrence_id.is_empty());
    // No METHOD in the text: left empty (the mail part's own method= decides).
    assert_eq!(parse_invite(&INVITE.replace("METHOD:REQUEST\r\n", "")).unwrap().method, "");
    assert!(parse_invite("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").is_none());
}

#[test]
fn invitation_replies_carry_the_uid_and_answer() {
    let inv = parse_invite(INVITE).unwrap();
    let ics = reply_ics(&inv, "isaac@example.org", "Isaac Lehman", "ACCEPTED", 1_000);
    assert!(ics.contains("METHOD:REPLY\r\n") && ics.contains("UID:7kq2c4ab@google.com\r\n") && ics.contains("SEQUENCE:2\r\n"), "{ics}");
    assert!(ics.contains("ATTENDEE;PARTSTAT=ACCEPTED;CN=\"Isaac Lehman\":mailto:isaac@example.org"), "{ics}");
    assert!(ics.contains("ORGANIZER;CN=\"Ruiz, Ana\":mailto:ana@example.com"), "{ics}");
    assert!(ics.lines().all(|l| l.len() <= 76), "folded at 75 bytes");
    // Someone else's title, UID or recurrence can't add lines to the reply.
    let mut sneaky = inv.clone();
    sneaky.title = "Lunch\rATTENDEE:mailto:x@evil.example".into();
    sneaky.uid = "u1\r\nATTENDEE:mailto:y@evil.example".into();
    let bad = reply_ics(&sneaky, "isaac@example.org", "Isaac Lehman", "ACCEPTED", 1_000);
    assert!(!bad.contains("\rATTENDEE:mailto:x@") && !bad.contains("\nATTENDEE:mailto:y@"), "{bad}");
    assert_eq!(bad.lines().filter(|l| l.starts_with("ATTENDEE")).count(), 1, "only the user's own line: {bad}");
    // Read back: the same event, with only the user's answer.
    let back = parse_invite(&ics).unwrap();
    assert_eq!((back.method.as_str(), back.uid.as_str(), back.start, back.end), ("REPLY", inv.uid.as_str(), inv.start, inv.end));
    let people: Vec<(&str, &str)> = back.attendees.iter().map(|a| (a.email.as_str(), a.partstat.as_str())).collect();
    assert_eq!(people, vec![("isaac@example.org", "ACCEPTED")]);
    for (answer, want) in [("TENTATIVE", "PARTSTAT=TENTATIVE"), ("DECLINED", "PARTSTAT=DECLINED")] {
        assert!(reply_ics(&inv, "isaac@example.org", "", answer, 1_000).contains(&format!("ATTENDEE;{want}:mailto:isaac@example.org")));
    }
    // A long title is folded, and unfolds back whole.
    let long = Invite { title: "A ".repeat(80) + "end", ..inv.clone() };
    let ics = reply_ics(&long, "isaac@example.org", "Isaac", "ACCEPTED", 1_000);
    assert!(ics.lines().all(|l| l.len() <= 76));
    assert_eq!(parse_invite(&ics).unwrap().title, long.title);
}

#[test]
fn events_are_found_by_uid() {
    struct Plain;
    impl crate::crypto::Protector for Plain {
        fn protect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
        fn unprotect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
    }
    let dir = std::env::temp_dir().join(format!("sulcusai-cal-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let c = crate::crypto::Vault::open(&dir.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap();
    let conn = db::open(&dir.join("t.db")).unwrap();
    let ev = |id: &str, uid: &str| Event { id: id.into(), account_id: None, calendar: None, start: 1_000, end: 2_000, all_day: false, data: EventData { title: id.into(), uid: uid.into(), ..Default::default() } };
    insert(&conn, &c, &ev("mine", "1234@sulcusai")).unwrap();
    assert!(find_by_uid(&conn, &c, "7kq2c4ab@google.com").is_none(), "not on the calendar yet: it's added");
    // Gmail put the invitation on the calendar itself (seen after a sync).
    insert(&conn, &c, &ev("gmail", "7kq2c4ab@google.com")).unwrap();
    assert_eq!(find_by_uid(&conn, &c, " 7kq2c4ab@google.com ").unwrap().id, "gmail", "there already: not added twice");
    assert!(find_by_uid(&conn, &c, "").is_none());
}

#[test]
fn free_time_skips_busy_blocks() {
    let day = NaiveDate::from_ymd_opt(2026, 10, 9).unwrap();
    let at = |h: u32, m: u32| Local.from_local_datetime(&day.and_hms_opt(h, m, 0).unwrap()).unwrap().timestamp_millis();
    let busy = [(at(10, 0), at(11, 0)), (at(10, 30), at(12, 0)), (at(15, 0), at(15, 20))];
    let slots = free_slots(&busy, at(0, 0), at(23, 59), 30, 9, 17);
    assert_eq!(slots, vec![(at(9, 0), at(10, 0)), (at(12, 0), at(15, 0)), (at(15, 20), at(17, 0))]);
    let long = free_slots(&busy, at(0, 0), at(23, 59), 120, 9, 17);
    assert_eq!(long, vec![(at(12, 0), at(15, 0))]);
}

#[test]
fn webdav_answers_are_read() {
    let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
 <d:response><d:href>/dav/calendars/user/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
 <d:response><d:href>/dav/calendars/user/work/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>Work &amp; school</d:displayname></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  <d:propstat><d:prop><x:calendar-color xmlns:x="http://apple.com/ns/ical/"/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>
 <d:response><d:href>/p/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/principals/user/</d:href></d:current-user-principal></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
</d:multistatus>"#;
    let r = parse_multistatus(xml);
    assert_eq!(r.len(), 3);
    assert!(!r[0].types.contains(&"calendar".to_string()));
    assert!(r[1].types.contains(&"calendar".to_string()));
    assert_eq!(r[1].props.get("displayname").map(String::as_str), Some("Work & school"));
    assert_eq!(r[2].hrefs.get("current-user-principal").map(String::as_str), Some("/principals/user/"));
}
