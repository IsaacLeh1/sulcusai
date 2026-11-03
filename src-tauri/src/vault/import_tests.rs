use std::io::Write;

use super::import::{import_any, import_csv, parse_csv, parse_time, Imported, Source};
use super::import_formats::{bitwarden_json, keepass_xml, one_password_1pux};
use super::VaultError;

fn one(imported: &Imported) -> (&str, &str, &str, &str, &str) {
    assert_eq!(imported.entries.len(), 1, "{imported:?}");
    let e = &imported.entries[0];
    (e.origin.as_str(), e.username.as_str(), e.password.as_str(), e.name.as_str(), e.notes.as_str())
}

const CHROME: &str = "name,url,username,password,note\r\nGitHub,https://github.com/login,octo,\"pa,ss\"\"word \",\"multi\nline\"\r\n";
const FIREFOX: &str = "\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\",\"timeCreated\",\"timeLastUsed\",\"timePasswordChanged\"\n\
\"https://fx.example.com\",\"fox\",\"fxpw\",,\"https://fx.example.com\",\"{abc}\",\"1600000000000\",\"1700000000000\",\"1650000000000\"\n";
const KEEPASS_CSV: &str = "\u{feff}\"Account\",\"Login Name\",\"Password\",\"Web Site\",\"Comments\"\n\"Bank\",\"me\",\"bankpw\",\"https://bank.example.com\",\"a note\"\n";
const KEEPASSXC_CSV: &str = "\"Group\",\"Title\",\"Username\",\"Password\",\"URL\",\"Notes\",\"TOTP\",\"Icon\",\"Last Modified\",\"Created\"\n\
\"Root\",\"Mail\",\"me\",\"mailpw\",\"https://mail.example.com\",\"\",\"\",\"0\",\"2024-01-02T03:04:05Z\",\"2023-01-02T03:04:05Z\"\n";
const BITWARDEN_CSV: &str = "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n\
,,login,Site,,,,https://site.example.com,me,sitepw,\n\
,,note,Secure note,text,,,,,,\n";
const ONEPASSWORD_CSV: &str = "Title,Url,Username,Password,OTPAuth,Favorite,Archived,Tags,Notes\nShop,https://shop.example.com,me,shoppw,,false,false,,\n";
const BITWARDEN_JSON: &str = r#"{"encrypted":false,"folders":[],"items":[
 {"type":1,"name":"Site","notes":null,"login":{"username":"me","password":"bwpw","uris":[{"match":null,"uri":"https://bw.example.com"}]},
  "creationDate":"2023-01-01T00:00:00.000Z","revisionDate":"2024-01-01T00:00:00.000Z"},
 {"type":2,"name":"note","notes":"x","secureNote":{"type":0}},
 {"type":1,"name":"gone","login":{"password":"x"},"deletedDate":"2024-01-01T00:00:00Z"}]}"#;
const KEEPASS_XML: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<KeePassFile>
 <Meta><Generator>KeePass</Generator><RecycleBinUUID>BIN==</RecycleBinUUID></Meta>
 <Root>
  <Group><UUID>ROOT==</UUID><Name>Database</Name>
   <Entry>
    <UUID>E1==</UUID>
    <Times><CreationTime>2023-01-01T00:00:00Z</CreationTime><LastModificationTime>2024-01-01T00:00:00Z</LastModificationTime></Times>
    <String><Key>Title</Key><Value>Forum</Value></String>
    <String><Key>UserName</Key><Value>kp</Value></String>
    <String><Key>Password</Key><Value ProtectInMemory="True">p&amp;w&lt;1</Value></String>
    <String><Key>URL</Key><Value>https://forum.example.com/login</Value></String>
    <String><Key>Notes</Key><Value><![CDATA[line <one>]]></Value></String>
    <History>
     <Entry><String><Key>Password</Key><Value>an old one</Value></String><String><Key>URL</Key><Value>https://old.example.com</Value></String></Entry>
    </History>
   </Entry>
   <Group><UUID>BIN==</UUID><Name>Recycle Bin</Name>
    <Entry><String><Key>Password</Key><Value>binned</Value></String></Entry>
   </Group>
  </Group>
 </Root>
</KeePassFile>"#;
const ONEPUX_DATA: &str = r#"{"accounts":[{"attrs":{"name":"me"},"vaults":[{"attrs":{"name":"Personal"},"items":[
 {"uuid":"a","createdAt":1600000000,"updatedAt":1700000000,"state":"active","categoryUuid":"001",
  "details":{"loginFields":[{"value":"op","designation":"username"},{"value":"oppw","designation":"password"}],"notesPlain":"n"},
  "overview":{"title":"Shop","url":"https://op.example.com"}},
 {"uuid":"b","categoryUuid":"005","details":{"password":"lonepw"},"overview":{"title":"Wifi","urls":[{"url":"https://wifi.example.com"}]}},
 {"uuid":"c","categoryUuid":"002","details":{},"overview":{"title":"Card"}}]}]}]}"#;

fn onepux(data: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("export.attributes", zip::write::SimpleFileOptions::default()).unwrap();
    zip.write_all(b"{\"version\":3}").unwrap();
    zip.start_file("export.data", zip::write::SimpleFileOptions::default()).unwrap();
    zip.write_all(data.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

#[test]
fn chrome_and_edge_csv() {
    let r = import_any(CHROME.as_bytes()).unwrap();
    assert_eq!(r.source, Some(Source::ChromeOrEdge));
    assert_eq!(one(&r), ("https://github.com/login", "octo", "pa,ss\"word ", "GitHub", "multi\nline"));
}

#[test]
fn firefox_csv_with_times() {
    let r = import_any(FIREFOX.as_bytes()).unwrap();
    assert_eq!(r.source, Some(Source::Firefox));
    assert_eq!(one(&r).2, "fxpw");
    assert_eq!(r.entries[0].created_ms, 1_600_000_000_000);
    assert_eq!(r.entries[0].changed_ms, 1_650_000_000_000);
}

#[test]
fn keepass_csv_and_keepassxc_csv() {
    let r = import_any(KEEPASS_CSV.as_bytes()).unwrap();
    assert_eq!(r.source, Some(Source::KeePass));
    assert_eq!(one(&r), ("https://bank.example.com", "me", "bankpw", "Bank", "a note"));
    let r = import_any(KEEPASSXC_CSV.as_bytes()).unwrap();
    assert_eq!(r.source, Some(Source::KeePassXc));
    assert_eq!(one(&r).2, "mailpw");
    assert_eq!(r.entries[0].changed_ms, parse_time("2024-01-02T03:04:05Z").unwrap());
}

#[test]
fn bitwarden_csv_and_json() {
    let r = import_any(BITWARDEN_CSV.as_bytes()).unwrap();
    assert_eq!(r.source, Some(Source::Bitwarden));
    assert_eq!(one(&r), ("https://site.example.com", "me", "sitepw", "Site", ""));
    assert_eq!(r.skipped, 1);

    let r = import_any(BITWARDEN_JSON.as_bytes()).unwrap();
    assert_eq!(one(&r), ("https://bw.example.com", "me", "bwpw", "Site", ""));
    assert_eq!(r.skipped, 2);
    assert!(r.entries[0].changed_ms > r.entries[0].created_ms);

    let enc = r#"{"encrypted":true,"encKeyValidation_DO_NOT_EDIT":"x","data":"y"}"#;
    assert!(matches!(import_any(enc.as_bytes()), Err(VaultError::BadImport(m)) if m.contains("encrypted")));
    assert!(import_any(b"{\"hello\":1}").is_err());
}

#[test]
fn onepassword_csv_and_1pux() {
    let r = import_any(ONEPASSWORD_CSV.as_bytes()).unwrap();
    assert_eq!(r.source, Some(Source::OnePassword));
    assert_eq!(one(&r).2, "shoppw");

    let r = import_any(&onepux(ONEPUX_DATA)).unwrap();
    assert_eq!(r.source, Some(Source::OnePassword));
    assert_eq!(r.entries.len(), 2);
    assert_eq!(r.skipped, 1);
    let e = &r.entries[0];
    assert_eq!((e.origin.as_str(), e.username.as_str(), e.password.as_str(), e.notes.as_str()), ("https://op.example.com", "op", "oppw", "n"));
    assert_eq!((e.created_ms, e.changed_ms), (1_600_000_000_000, 1_700_000_000_000));
    assert_eq!((r.entries[1].origin.as_str(), r.entries[1].password.as_str()), ("https://wifi.example.com", "lonepw"));
}

#[test]
fn keepass_xml_skips_history_and_recycle_bin() {
    let r = import_any(KEEPASS_XML.as_bytes()).unwrap();
    assert_eq!(r.source, Some(Source::KeePass));
    assert_eq!(one(&r), ("https://forum.example.com/login", "kp", "p&w<1", "Forum", "line <one>"));
    assert_eq!(r.skipped, 1);
    assert_eq!(r.entries[0].changed_ms, parse_time("2024-01-01T00:00:00Z").unwrap());
    assert!(keepass_xml("<html><body/></html>").is_err());
}

#[test]
fn keepass_binary_times() {
    use base64::Engine;
    let secs_since_year_one: i64 = 62_135_596_800 + 1_700_000_000;
    let b64 = base64::engine::general_purpose::STANDARD.encode(secs_since_year_one.to_le_bytes());
    let xml = KEEPASS_XML.replace("2024-01-01T00:00:00Z", &b64);
    let r = keepass_xml(&xml).unwrap();
    assert_eq!(r.entries[0].changed_ms, 1_700_000_000_000);
}

#[test]
fn files_without_passwords_are_refused() {
    assert!(matches!(import_any(b""), Err(VaultError::BadImport(_))));
    assert!(matches!(import_any(b"a,b,c\n1,2,3\n"), Err(VaultError::BadImport(_))));
    assert!(import_any(b"PK\x03\x04garbage").is_err());
    assert!(import_any(&vec![b'a'; super::import::MAX_IMPORT_BYTES + 1]).is_err());
    let deep = format!("<KeePassFile>{}", "<Group>".repeat(10_000));
    assert!(keepass_xml(&deep).is_err());
}

#[test]
fn csv_edge_cases() {
    assert_eq!(parse_csv("a,\"b\nc\",d\re,f\r\n"), vec![vec!["a", "b\nc", "d"], vec!["e", "f"]]);
    assert_eq!(parse_csv("x\"y,\"unclosed"), vec![vec!["x\"y", "unclosed"]]);
    assert_eq!(parse_csv("\n\n,,\n"), Vec::<Vec<String>>::new());
    let r = import_csv("password,url\nonly-a-password,\n").unwrap();
    assert_eq!(r.source, Some(Source::OtherCsv));
    assert_eq!(r.entries[0].password, "only-a-password");
}

#[test]
fn times_in_every_export_form() {
    assert_eq!(parse_time("1700000000"), Some(1_700_000_000_000));
    assert_eq!(parse_time("1700000000123"), Some(1_700_000_000_123));
    assert_eq!(parse_time("2023-11-14 22:13:20"), Some(1_700_000_000_000));
    assert_eq!(parse_time("2023-11-14T22:13:20.000Z"), Some(1_700_000_000_000));
    assert_eq!(parse_time("99999999999999999999"), None);
    assert_eq!(parse_time("yesterday"), None);
}

/// A small fixed-seed generator, so a failure here can be replayed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const NASTY: &[&[u8]] = &[
    b"\"", b",", b"\n", b"\r", b"<", b">", b"&", b"&#x0;", b"&bogus;", b"]]>", b"<![CDATA[", b"{", b"}", b"[", b"]", b":", b"\\",
    b"\xff", b"\xc3", b"\0", b"<History>", b"</Entry>", b"<String><Key>Password</Key><Value>", b"\"items\":", b"null", b"1e999",
];

fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut b = seed.to_vec();
    for _ in 0..=rng.below(6) {
        match rng.below(5) {
            0 if !b.is_empty() => {
                let i = rng.below(b.len());
                b[i] = rng.next() as u8;
            }
            1 => {
                let i = rng.below(b.len() + 1);
                let piece = NASTY[rng.below(NASTY.len())];
                b.splice(i..i, piece.iter().copied());
            }
            2 if !b.is_empty() => {
                let i = rng.below(b.len());
                b.truncate(i);
            }
            3 if b.len() > 2 => {
                let i = rng.below(b.len() - 1);
                let j = i + rng.below(b.len() - i);
                b.drain(i..j);
            }
            _ => {
                let i = rng.below(b.len() + 1);
                let j = i + rng.below(b.len() - i + 1);
                let copy = b[i..j].to_vec();
                b.splice(i..i, copy);
            }
        }
    }
    b
}

/// Every parser must return, never panic, on garbled versions of each
/// format and on random bytes.
#[test]
fn parsers_never_panic_on_garbled_input() {
    let pux = onepux(ONEPUX_DATA);
    let seeds: Vec<&[u8]> = vec![
        CHROME.as_bytes(),
        FIREFOX.as_bytes(),
        KEEPASS_CSV.as_bytes(),
        KEEPASSXC_CSV.as_bytes(),
        BITWARDEN_CSV.as_bytes(),
        ONEPASSWORD_CSV.as_bytes(),
        BITWARDEN_JSON.as_bytes(),
        KEEPASS_XML.as_bytes(),
        &pux,
    ];
    let mut rng = Rng(0x5EED_CAFE_F00D_0001);
    for seed in &seeds {
        for _ in 0..400 {
            let input = mutate(&mut rng, seed);
            let _ = import_any(&input);
            let text = String::from_utf8_lossy(&input);
            let _ = import_csv(&text);
            let _ = keepass_xml(&text);
            let _ = bitwarden_json(&text);
        }
    }
    for _ in 0..300 {
        let input = mutate(&mut rng, &pux);
        let _ = one_password_1pux(&input);
    }
    for _ in 0..2000 {
        let len = rng.below(400);
        let input: Vec<u8> = (0..len)
            .map(|_| if rng.below(3) == 0 { NASTY[rng.below(NASTY.len())][0] } else { rng.next() as u8 })
            .collect();
        let _ = import_any(&input);
        let text = String::from_utf8_lossy(&input);
        let _ = import_csv(&text);
        let _ = keepass_xml(&text);
        let _ = bitwarden_json(&text);
    }
}

#[test]
fn hostile_structures_are_refused_cleanly() {
    let deep_json = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(bitwarden_json(&deep_json).is_err());
    let wrong_types = r#"{"items":[1,"x",null,{"type":"1","login":5},{"type":1,"login":{"password":7,"uris":"no"}}]}"#;
    assert_eq!(bitwarden_json(wrong_types).unwrap().entries.len(), 0);
    let wrong_1pux = r#"{"accounts":[{"vaults":"x"},{"vaults":[{"items":[{"details":{"loginFields":"no"}}]}]}]}"#;
    assert_eq!(one_password_1pux(&onepux(wrong_1pux)).unwrap().entries.len(), 0);
}
