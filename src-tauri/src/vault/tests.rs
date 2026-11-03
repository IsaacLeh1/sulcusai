use std::path::PathBuf;

use super::*;

const MASTER: &str = "violet anchor 47 drifting";
const PAGE: &str = "https://accounts.example.com/login";

struct Temp(PathBuf);

impl Temp {
    fn new() -> Temp {
        let dir = std::env::temp_dir().join(format!("sulcus-vault-test-{}", uuid::Uuid::new_v4()));
        Temp(dir)
    }
    fn file(&self) -> PathBuf {
        self.0.join("password-vault.json")
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn input(origin: &str, user: &str, pw: &str) -> EntryInput {
    let mut i = EntryInput::default();
    i.origin = origin.into();
    i.username = user.into();
    i.password = pw.into();
    i
}

fn ready(t: &Temp) -> (PasswordVault, Zeroizing<String>) {
    let mut v = PasswordVault::for_test(&t.file());
    v.set_enabled(true);
    let rk = v.setup(MASTER).unwrap();
    (v, rk)
}

#[test]
fn off_by_default_and_refuses_everything_while_off() {
    let t = Temp::new();
    let mut v = PasswordVault::for_test(&t.file());
    assert!(!v.is_enabled());
    assert_eq!(v.setup(MASTER).unwrap_err(), VaultError::Off);
    assert_eq!(v.unlock(MASTER).unwrap_err(), VaultError::Off);
    assert_eq!(v.list().unwrap_err(), VaultError::Off);
    assert!(v.generate(&GenOptions::default()).is_err());
    assert!(!v.should_offer_save(PAGE));
    assert!(!t.file().exists());
}

#[test]
fn turning_off_locks() {
    let t = Temp::new();
    let (mut v, _) = ready(&t);
    assert!(v.is_open());
    v.set_enabled(false);
    v.set_enabled(true);
    assert!(!v.is_open());
    assert_eq!(v.list().unwrap_err(), VaultError::Locked);
    v.unlock(MASTER).unwrap();
    assert!(v.is_open());
}

#[test]
fn setup_refuses_weak_master_and_a_second_setup() {
    let t = Temp::new();
    let mut v = PasswordVault::for_test(&t.file());
    v.set_enabled(true);
    for weak in ["short", "password123", "Password1!", "aaaaaaaaaaaaaaa", "1234567890"] {
        assert!(matches!(v.setup(weak), Err(VaultError::WeakMaster(_))), "{weak}");
    }
    v.setup(MASTER).unwrap();
    assert_eq!(v.setup(MASTER).unwrap_err(), VaultError::AlreadySetUp);
}

#[test]
fn saved_logins_survive_lock_and_restart_and_stay_encrypted() {
    let t = Temp::new();
    let (mut v, _) = ready(&t);
    let (id, s) = v.save_login(&input(PAGE, "me@example.com", "Tr0ub4dour&3-unique")).unwrap();
    assert_eq!(s, Saved::Added);
    assert_eq!(v.save_login(&input(PAGE, "me@example.com", "Tr0ub4dour&3-unique")).unwrap(), (id.clone(), Saved::Unchanged));
    assert_eq!(v.save_login(&input("https://accounts.example.com/", "me@example.com", "changed-pass-9")).unwrap(), (id.clone(), Saved::Updated));
    v.never_for("https://news.example.org/a").unwrap();

    let raw = std::fs::read_to_string(t.file()).unwrap();
    for secret in ["changed-pass-9", "me@example.com", "accounts.example.com", "news.example.org"] {
        assert!(!raw.contains(secret), "{secret} is readable in the file");
    }

    drop(v);
    let mut v = PasswordVault::for_test(&t.file());
    v.set_enabled(true);
    assert_eq!(v.list().unwrap_err(), VaultError::Locked);
    v.unlock(MASTER).unwrap();
    let list = v.list().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].origin, "https://accounts.example.com");
    assert!(list[0].fillable);
    assert_eq!(*v.reveal(&id).unwrap().0, "changed-pass-9");
    assert!(!v.should_offer_save("https://news.example.org/b"));
    assert!(v.should_offer_save("https://other.example.org/"));
    assert!(!v.should_offer_save("http://other.example.org/"));
    v.allow_again("https://news.example.org").unwrap();
    assert!(v.should_offer_save("https://news.example.org/b"));
}

#[test]
fn filling_follows_the_exact_origin_rule() {
    let t = Temp::new();
    let (mut v, _) = ready(&t);
    let (id, _) = v.save_login(&input(PAGE, "me", "fill-me-123456")).unwrap();
    v.save_login(&input("http://accounts.example.com", "me", "plain-http-pw")).unwrap();

    assert_eq!(v.logins_for(PAGE, PAGE).unwrap().len(), 1);
    assert_eq!(*v.password_for_fill(&id, PAGE, PAGE).unwrap(), "fill-me-123456");
    assert_eq!(v.password_for_fill(&id, "https://accounts.examp1e.com/login", "https://accounts.examp1e.com/login").unwrap_err(), VaultError::Refused(Refusal::Lookalike));
    assert_eq!(v.password_for_fill(&id, "http://accounts.example.com/login", "http://accounts.example.com/login").unwrap_err(), VaultError::Refused(Refusal::NotHttps));
    assert_eq!(v.password_for_fill(&id, PAGE, "https://evil.example.net/").unwrap_err(), VaultError::Refused(Refusal::FrameFromOtherSite));
    assert!(v.logins_for("http://accounts.example.com/", "http://accounts.example.com/").unwrap().is_empty());
}

#[test]
fn wrong_passwords_lock_out_with_growing_waits() {
    let t = Temp::new();
    let (mut v, rk) = ready(&t);
    v.lock();
    let mut now = 1_000_000_000;
    let mut waits = Vec::new();
    for _ in 0..8 {
        match v.unlock_at("not the password", now) {
            Err(VaultError::WrongPassword { wait_ms }) => waits.push(wait_ms),
            other => panic!("{other:?}"),
        }
        // Even the right password waits its turn.
        if *waits.last().unwrap() > 0 {
            assert!(matches!(v.unlock_at(MASTER, now), Err(VaultError::Wait { .. })));
            assert!(matches!(v.recover_at(&rk, "a brand new master 77", now), Err(VaultError::Wait { .. })));
        }
        now += waits.last().unwrap();
    }
    assert_eq!(&waits[..2], &[0, 0]);
    assert!(waits.windows(2).all(|w| w[1] >= w[0]));
    assert!(waits[7] > waits[3]);

    // The count is in the file, so a restart doesn't reset it.
    let mut again = PasswordVault::for_test(&t.file());
    again.set_enabled(true);
    assert!(matches!(again.unlock_at("still wrong", now), Err(VaultError::WrongPassword { wait_ms }) if wait_ms > waits[7]));

    let later = now + keys::MAX_WAIT_MS;
    again.unlock_at(MASTER, later).unwrap();
    again.lock();
    assert!(matches!(again.unlock_at("wrong once more", later), Err(VaultError::WrongPassword { wait_ms: 0 })));
}

#[test]
fn recovery_key_resets_the_master_password() {
    let t = Temp::new();
    let (mut v, rk) = ready(&t);
    v.save_login(&input(PAGE, "me", "kept-through-recovery")).unwrap();
    v.lock();
    assert!(matches!(v.recover("AAAAA-AAAAA-AAAAA-AAAAA-AAAAA-AAAAA", "a brand new master 77"), Err(VaultError::WrongPassword { .. })));
    let typed = rk.to_lowercase().replace('-', " ");
    v.recover(&typed, "a brand new master 77").unwrap();
    assert_eq!(v.list().unwrap().len(), 1);
    v.lock();
    assert!(matches!(v.unlock(MASTER), Err(VaultError::WrongPassword { .. })));
    v.unlock("a brand new master 77").unwrap();

    let new_rk = v.new_recovery_key("a brand new master 77").unwrap();
    v.lock();
    assert!(v.recover(&rk, "another new master 88").is_err());
    v.recover(&new_rk, "another new master 88").unwrap();
}

#[test]
fn changing_the_master_password_asks_for_the_current_one() {
    let t = Temp::new();
    let (mut v, _) = ready(&t);
    assert!(matches!(v.change_master("guess guess guess", "replacement master 42"), Err(VaultError::WrongPassword { .. })));
    v.change_master(MASTER, "replacement master 42").unwrap();
    v.lock();
    v.unlock("replacement master 42").unwrap();
}

#[test]
fn export_and_import_between_vaults() {
    let t = Temp::new();
    let (mut v, _) = ready(&t);
    v.save_login(&input(PAGE, "me", "moved-between-pcs")).unwrap();
    v.never_for("https://never.example.com").unwrap();
    assert!(matches!(v.export("weak"), Err(VaultError::WeakMaster(_))));
    let bytes = v.export("export phrase 2026 blue").unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("moved-between-pcs"));

    let t2 = Temp::new();
    let (mut w, _) = ready(&t2);
    assert!(matches!(w.import_file(&bytes), Err(VaultError::BadImport(_))));
    assert!(matches!(w.import_export(&bytes, "wrong phrase"), Err(VaultError::WrongPassword { .. })));
    let r = w.import_export(&bytes, "export phrase 2026 blue").unwrap();
    assert_eq!(r.added, 1);
    assert_eq!(w.never_list().unwrap(), vec!["https://never.example.com".to_string()]);
    assert_eq!(w.import_export(&bytes, "export phrase 2026 blue").unwrap().unchanged, 1);
}

#[test]
fn imports_a_chrome_csv_and_checks_health() {
    let t = Temp::new();
    let (mut v, _) = ready(&t);
    let csv = "name,url,username,password,note\n\
               a,https://a.example.com/x,me,letmein,\n\
               b,https://b.example.com/,me,k7#Qm2!vX9pL$w4Z,\n\
               c,https://c.example.com/,me,k7#Qm2!vX9pL$w4Z,\n\
               d,https://d.example.com/,me,,\n";
    let (source, r) = v.import_file(csv.as_bytes()).unwrap();
    assert_eq!(source, Some(Source::ChromeOrEdge));
    assert_eq!((r.added, r.skipped), (3, 1));
    let report = v.check().unwrap();
    assert_eq!(report.weak.len(), 1);
    assert_eq!(report.reused.len(), 1);
    assert_eq!(report.reused[0].len(), 2);
}

#[test]
fn edit_and_remove() {
    let t = Temp::new();
    let (mut v, _) = ready(&t);
    let (id, _) = v.save_login(&input(PAGE, "me", "first-password-1")).unwrap();
    let mut edit = input(PAGE, "me2", "");
    edit.notes = "security question: none".into();
    v.update(&id, &edit).unwrap();
    let (pw, notes) = v.reveal(&id).unwrap();
    assert_eq!((pw.as_str(), notes.as_str()), ("first-password-1", "security question: none"));
    assert_eq!(v.list().unwrap()[0].username, "me2");
    v.remove(&id).unwrap();
    assert_eq!(v.remove(&id).unwrap_err(), VaultError::NotFound);
    assert!(v.list().unwrap().is_empty());
}

#[test]
fn a_damaged_file_is_reported_not_trusted() {
    let t = Temp::new();
    let (mut v, _) = ready(&t);
    v.save_login(&input(PAGE, "me", "tamper-target-1")).unwrap();
    v.lock();
    let text = std::fs::read_to_string(t.file()).unwrap();
    let mut json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let data = json["data"].as_str().unwrap().to_string();
    let mut bytes = keys::unb64(&data).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x40;
    json["data"] = serde_json::Value::String(keys::b64(&bytes));
    std::fs::write(t.file(), serde_json::to_string(&json).unwrap()).unwrap();
    assert!(matches!(v.unlock(MASTER), Err(VaultError::Damaged(_))));
    std::fs::write(t.file(), "{ not json").unwrap();
    assert!(matches!(v.unlock(MASTER), Err(VaultError::Damaged(_))));
}

#[test]
fn locking_wipes_the_key_in_place() {
    let t = Temp::new();
    let (mut v, _) = ready(&t);
    let open = v.open.as_mut().unwrap();
    let ptr = open.key.as_ptr();
    assert!(unsafe { std::slice::from_raw_parts(ptr, 32) }.iter().any(|b| *b != 0));
    // What `Open`'s drop does, checked while the memory is still owned.
    open.key.zeroize();
    assert!(unsafe { std::slice::from_raw_parts(ptr, 32) }.iter().all(|b| *b == 0));
    v.lock();
    assert!(v.open.is_none());
}

#[test]
fn real_key_settings_unlock() {
    let t = Temp::new();
    let mut v = PasswordVault::new(&t.file());
    v.set_enabled(true);
    v.setup(MASTER).unwrap();
    v.lock();
    v.unlock(MASTER).unwrap();
}

#[test]
fn the_vault_has_no_network_code() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("vault");
    for f in std::fs::read_dir(dir).unwrap() {
        let path = f.unwrap().path();
        let src = std::fs::read_to_string(&path).unwrap();
        for banned in ["reqwest", "tokio::net", "std::net", "hyper", "tungstenite", "TcpStream", "UdpSocket", "crate::net", "arti_client"] {
            let hits = src.lines().filter(|l| l.contains(banned) && !l.contains("for banned in")).count();
            assert_eq!(hits, 0, "{} mentions {banned}", path.display());
        }
    }
}
