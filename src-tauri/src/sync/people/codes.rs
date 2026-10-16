// SPDX-License-Identifier: AGPL-3.0-only
//! Codes for linking with people, and who linked with each.
//!
//! - A code doesn't expire. It works for as many people as the user allows
//!   (one by default) until it's turned off or deleted, and stops after 10
//!   wrong tries, so it can't be guessed.
//! - Each code lists who linked with it; anyone can be removed, or blocked
//!   so they can't link again with any code.
//! - The first two characters say which of the user's codes is meant (they
//!   travel in the clear); all eight are the secret both apps prove they
//!   share (SPAKE2), so a wrong guess learns nothing.
//! - Linking reports each step (`people:connecting`) and, when it fails, why
//!   in plain words; the app whose code it is hears about each try
//!   (`people:attempt`).
//! - Windows Firewall: virtual networks such as Radmin VPN count as Public
//!   networks, where SulcusAI often isn't allowed in. The Connect dialog
//!   checks, and can fix it with Windows' permission prompt.

use serde_json::json;

use super::*;

/// Wrong tries before a code stops working.
const WRONG_LIMIT: u32 = 10;
/// The characters that say which code is meant.
const TAG: usize = 2;
/// Linking gives up after this long, whatever is happening.
const LINK_LIMIT: Duration = Duration::from_secs(60);
const NOT_TAKING: &str = "That code isn't taking connections right now. Ask them to check it in People → Connect.";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Invite {
    pub id: String,
    /// The code, sealed with the data key.
    pub code: String,
    pub label: String,
    /// How many people may link with it.
    pub max: u32,
    /// Who linked with it.
    pub used: Vec<Use>,
    /// Wrong codes tried against it.
    pub wrong: u32,
    pub closed: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Use {
    pub id: String,
    pub name: String,
    pub at: i64,
}

/// Someone who can't link with this app again (until unblocked).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Blocked {
    pub id: String,
    pub name: String,
    pub at: i64,
}

impl Invite {
    pub(super) fn open(&self) -> bool {
        self.why_closed().is_none()
    }

    pub(super) fn why_closed(&self) -> Option<&'static str> {
        if self.closed {
            Some("Turned off.")
        } else if self.wrong >= WRONG_LIMIT {
            Some("Stopped after too many wrong tries. Make a new code.")
        } else if self.used.len() as u32 >= self.max.max(1) {
            Some("Used up.")
        } else {
            None
        }
    }
}

/// Some code is taking links, so this app listens and says so on its
/// network. Locks the database: never call it while holding it.
pub(in crate::sync) fn showing_code(state: &AppState) -> bool {
    let conn = state.db.lock().unwrap();
    features::is_on(&conn, Feature::People) && book(&conn).invites.iter().any(Invite::open)
}

pub(super) fn is_blocked(b: &Book, id: &str) -> bool {
    b.blocked.iter().any(|x| x.id == id)
}

/// "K7Q2-9XMB".
fn shown(code: &str) -> String {
    if code.len() == 8 { format!("{}-{}", &code[..4], &code[4..]) } else { code.to_string() }
}

/// A new code whose first characters differ from every open code's.
fn fresh_code(taken: &[String]) -> String {
    loop {
        let code = new_code();
        if !taken.iter().any(|t| t.starts_with(&code[..TAG])) {
            return code;
        }
    }
}

/// The app whose code it is: someone is trying it ("checking"), got it
/// wrong, linked, or was turned away.
fn attempt(state: &AppState, invite: &str, name: &str, result: &str) {
    emit(state, "people:attempt", json!({ "invite_id": invite, "name": name, "result": result, "at": db::now_ms() }));
}

// ---------- linking ----------

/// The app whose code it is: answers one attempt to link.
pub(in crate::sync) async fn pair_answer(state: &Arc<AppState>, mut stream: TcpStream, open: &Open, theirs: &str) -> Result<(), String> {
    let c = state.work_cipher()?;
    let refuse = |msg: &str| PairReply { error: Some(msg.into()), ..Default::default() };
    // Which code they typed: the first characters (an older app doesn't
    // send them; then it has to be the only open code).
    let picked: Result<(String, String), &str> = {
        let conn = state.db.lock().unwrap();
        let b = book(&conn);
        if !features::is_on(&conn, Feature::People) || !safe_id(&open.id) || is_blocked(&b, &open.id) {
            Err(NOT_TAKING)
        } else {
            let codes: Vec<(String, String)> = b.invites.iter().filter(|i| i.open()).filter_map(|i| c.decrypt(&i.code).ok().map(|code| (i.id.clone(), code))).collect();
            let tag = open.tag.as_deref().map(|t| t.to_uppercase());
            let matching: Vec<(String, String)> = match &tag {
                Some(t) => codes.into_iter().filter(|(_, code)| code.starts_with(t.as_str())).collect(),
                None => codes,
            };
            match matching.as_slice() {
                [one] => Ok(one.clone()),
                [] => Err(NOT_TAKING),
                _ => Err("Update SulcusAI to link with this code."),
            }
        }
    };
    let (invite, code) = match picked {
        Ok(x) => x,
        Err(e) => {
            if is_blocked(&book(&state.db.lock().unwrap()), &open.id) {
                attempt(state, "", &clean_name(&open.name), "refused");
            }
            return send_plain(&mut stream, &refuse(e)).await;
        }
    };
    let who = clean_name(&open.name);
    attempt(state, &invite, &who, "checking");
    let theirs = B64.decode(theirs).map_err(err)?;
    let me = device_id(&state.paths.data);
    let (spake, mine) = wire::pair_start(&code);
    send_plain(&mut stream, &PairReply { id: me.clone(), name: my_name(state), spake: B64.encode(&mine), error: None }).await?;
    let shared = wire::pair_finish(spake, &theirs)?;
    let t = transcript(&open.id, &me, &theirs, &mine);
    let proof = timeout(Duration::from_secs(30), wire::read_frame(&mut stream)).await.map_err(|_| "no proof".to_string())?.map_err(lost)?;
    if !wire::confirm_ok(&shared, "client", &t, &proof) {
        update(state, |b| {
            if let Some(i) = b.invites.iter_mut().find(|i| i.id == invite) {
                i.wrong += 1;
            }
        })?;
        state.log("people", &format!("{who} tried a wrong code"));
        attempt(state, &invite, &who, "wrong");
        changed(state);
        listen_again();
        return Err("wrong code".into());
    }
    wire::write_frame(&mut stream, &wire::confirm(&shared, "server", &t)).await.map_err(lost)?;
    // Their call right after says which port they take calls on.
    let name = keep(state, &c, &open.id, &open.name, &*wire::pair_key(&shared, &t), None)?;
    let label = update(state, |b| {
        b.invites.iter_mut().find(|i| i.id == invite).map(|i| {
            i.used.retain(|u| u.id != open.id);
            i.used.push(Use { id: open.id.clone(), name: name.clone(), at: db::now_ms() });
            i.label.clone()
        })
    })?;
    state.log("people", &format!("{name} linked with you{}", label.filter(|l| !l.is_empty()).map(|l| format!(" (code \"{l}\")")).unwrap_or_default()));
    attempt(state, &invite, &name, "linked");
    // A used-up code stops being announced.
    listen_again();
    changed(state);
    if let Some(app) = state.app.get() {
        crate::notify::show(app, "SulcusAI", &format!("{name} linked with you. You can chat in People."));
    }
    Ok(())
}

/// The app where the code is typed: links with the app showing it,
/// reporting each step.
pub(super) async fn pair_call(state: &Arc<AppState>, code: &str, addr: SocketAddr, step: &(dyn Fn(&str) + Send + Sync)) -> Result<String, String> {
    let c = state.work_cipher()?;
    let me = device_id(&state.paths.data);
    step("reach");
    let mut stream = connect(addr).await?;
    step("check");
    let (spake, mine) = wire::pair_start(code);
    let hello = Open { v: 1, id: me.clone(), name: my_name(state), nonce: None, pair: Some(B64.encode(&mine)), want: Some(LINK.into()), tag: Some(code[..TAG].to_string()) };
    send_plain(&mut stream, &hello).await?;
    let silent = "Their SulcusAI took the connection but didn't reply. Make sure you both have the latest version, then try again.";
    let reply: PairReply = recv_plain(&mut stream).await.map_err(|_| silent.to_string())?;
    if let Some(e) = reply.error {
        return Err(e);
    }
    if !safe_id(&reply.id) || reply.id == me {
        return Err(UNEXPECTED.into());
    }
    let theirs = B64.decode(&reply.spake).map_err(|_| UNEXPECTED.to_string())?;
    let shared = wire::pair_finish(spake, &theirs)?;
    let t = transcript(&me, &reply.id, &mine, &theirs);
    wire::write_frame(&mut stream, &wire::confirm(&shared, "client", &t)).await.map_err(lost)?;
    let wrong = "That code didn't match. Check the code they see in People → Connect and try again.";
    let proof = timeout(HANDSHAKE, wire::read_frame(&mut stream)).await.map_err(|_| wrong.to_string())?.map_err(|_| wrong.to_string())?;
    if !wire::confirm_ok(&shared, "server", &t, &proof) {
        return Err(wrong.into());
    }
    step("save");
    keep(state, &c, &reply.id, &reply.name, &*wire::pair_key(&shared, &t), Some(addr))
}

/// Links with whoever has `code`: at the address typed (or a nearby app's
/// id), or the only app nearby with a code showing. Each step comes as a
/// `people:connecting` event; the error says why it failed.
#[tauri::command]
pub async fn people_connect(state: AppStateRef<'_>, code: String, target: Option<String>) -> Result<String, String> {
    features::require(&state, Feature::People)?;
    state.cipher()?;
    let code = clean_code(&code).ok_or("A code is 8 letters and numbers, like K7Q2-9XMB.")?;
    let state = state.inner().clone();
    let s = state.clone();
    let step = move |step: &str| emit(&s, "people:connecting", json!({ "step": step }));
    step("find");
    let target = target.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let addr = match target {
        Some(t) => resolve(&t).await?,
        None => {
            let near = NEARBY.lock().unwrap();
            let showing: Vec<SocketAddr> = near.values().filter(|s| s.person && s.at.elapsed() < AWAY).map(|s| s.addr).collect();
            match showing.as_slice() {
                [one] => *one,
                [] => return Err("No one on this network has a code showing. Type the address they see in People → Connect (on Radmin VPN it starts with 26.).".into()),
                _ => return Err("More than one person here has a code showing. Pick which one.".into()),
            }
        }
    };
    let name = timeout(LINK_LIMIT, pair_call(&state, &code, addr, &step))
        .await
        .map_err(|_| "Linking took more than a minute, so it stopped. Try again; if it keeps happening, try the other way round (you show a code, they type it).".to_string())??;
    changed(&state);
    Ok(name)
}

// ---------- the codes ----------

#[derive(Debug, Serialize)]
pub struct UseView {
    id: String,
    name: String,
    at: i64,
    /// Still linked (not removed since).
    linked: bool,
    blocked: bool,
}

#[derive(Debug, Serialize)]
pub struct InviteView {
    id: String,
    code: String,
    label: String,
    max: u32,
    used: Vec<UseView>,
    wrong: u32,
    /// Taking links now.
    open: bool,
    /// Why not.
    why: Option<&'static str>,
    created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct BlockedView {
    id: String,
    name: String,
    at: i64,
}

pub(super) fn views(b: &Book, c: &Cipher) -> (Vec<InviteView>, Vec<BlockedView>) {
    let linked: HashSet<&str> = b.contacts.iter().map(|p| p.id.as_str()).collect();
    let mut codes: Vec<InviteView> = b
        .invites
        .iter()
        .map(|i| InviteView {
            id: i.id.clone(),
            code: shown(&c.decrypt_or(&i.code, "")),
            label: i.label.clone(),
            max: i.max.max(1),
            used: i
                .used
                .iter()
                .map(|u| UseView { id: u.id.clone(), name: u.name.clone(), at: u.at, linked: linked.contains(u.id.as_str()), blocked: is_blocked(b, &u.id) })
                .collect(),
            wrong: i.wrong,
            open: i.open(),
            why: i.why_closed(),
            created_at: i.created_at,
        })
        .collect();
    codes.sort_by_key(|i| (!i.open, std::cmp::Reverse(i.created_at)));
    let blocked = b.blocked.iter().map(|x| BlockedView { id: x.id.clone(), name: x.name.clone(), at: x.at }).collect();
    (codes, blocked)
}

fn cap(max: u32) -> u32 {
    max.clamp(1, 100)
}

/// A new code, good for `max` people (one by default).
#[tauri::command(async)]
pub fn people_new_code(state: AppStateRef, max: Option<u32>, label: Option<String>) -> Result<PeopleView, String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    let label: String = label.unwrap_or_default().trim().chars().filter(|c| !c.is_control()).take(40).collect();
    update(&state, |b| {
        let taken: Vec<String> = b.invites.iter().filter(|i| i.open()).filter_map(|i| c.decrypt(&i.code).ok()).collect();
        let code = fresh_code(&taken);
        b.invites.push(Invite { id: uuid::Uuid::new_v4().simple().to_string(), code: c.encrypt(&code), label, max: cap(max.unwrap_or(1)), created_at: db::now_ms(), ..Default::default() });
    })?;
    state.log("people", "Made a code for linking in People");
    listen_again();
    Ok(view(&state, &c))
}

/// Changes a code: how many people it's good for, its name, or on/off.
#[tauri::command(async)]
pub fn people_set_code(state: AppStateRef, id: String, max: Option<u32>, label: Option<String>, on: Option<bool>) -> Result<PeopleView, String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    update(&state, |b| {
        let i = b.invites.iter_mut().find(|i| i.id == id).ok_or("That code is gone.")?;
        if let Some(m) = max {
            i.max = cap(m);
        }
        if let Some(l) = label {
            i.label = l.trim().chars().filter(|c| !c.is_control()).take(40).collect();
        }
        if let Some(on) = on {
            i.closed = !on;
        }
        Ok::<_, String>(())
    })??;
    listen_again();
    changed(&state);
    Ok(view(&state, &c))
}

/// Deletes a code (the people who linked with it stay linked).
#[tauri::command(async)]
pub fn people_delete_code(state: AppStateRef, id: String) -> Result<PeopleView, String> {
    let c = state.cipher()?;
    update(&state, |b| b.invites.retain(|i| i.id != id))?;
    listen_again();
    changed(&state);
    Ok(view(&state, &c))
}

/// Blocks someone: they're removed and can't link with any code again, or
/// unblocks them (`block` false).
#[tauri::command(async)]
pub fn people_block(state: AppStateRef, id: String, name: Option<String>, block: bool) -> Result<PeopleView, String> {
    let c = state.cipher()?;
    if !safe_id(&id) {
        return Err("That isn't someone linked here.".into());
    }
    let who = update(&state, |b| {
        let who = b.contacts.iter().find(|p| p.id == id).map(|p| p.name.clone()).or(name.clone()).unwrap_or_else(|| "Someone".into());
        b.blocked.retain(|x| x.id != id);
        if block {
            b.blocked.push(Blocked { id: id.clone(), name: who.clone(), at: db::now_ms() });
        }
        who
    })?;
    if block {
        forget_contact(&state, &id)?;
        state.log("people", &format!("Blocked {who} in People"));
    } else {
        state.log("people", &format!("Unblocked {who} in People"));
    }
    changed(&state);
    Ok(view(&state, &c))
}

// ---------- Windows Firewall ----------

/// One network this PC is on, and whether people can reach SulcusAI there.
#[derive(Debug, Clone, Serialize)]
pub struct NetCheck {
    name: String,
    /// "Public", "Private" or "Domain".
    kind: String,
    allowed: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FirewallView {
    /// The check ran (Windows only).
    checked: bool,
    networks: Vec<NetCheck>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    Many(Vec<T>),
    One(T),
}

impl<T> OneOrMany<T> {
    fn list(self) -> Vec<T> {
        match self {
            OneOrMany::Many(v) => v,
            OneOrMany::One(x) => vec![x],
        }
    }
}

#[derive(Deserialize)]
struct RawNet {
    name: String,
    kind: String,
}

#[derive(Deserialize)]
struct RawRule {
    action: String,
    profile: String,
}

#[derive(Deserialize)]
struct RawProfile {
    name: String,
    enabled: String,
}

#[derive(Deserialize)]
struct RawCheck {
    nets: Option<OneOrMany<RawNet>>,
    rules: Option<OneOrMany<RawRule>>,
    profiles: Option<OneOrMany<RawProfile>>,
}

/// "Any", or a list like "Private, Public".
fn covers(profile: &str, kind: &str) -> bool {
    profile.trim() == "Any" || profile.split(',').any(|p| p.trim() == kind)
}

fn judge(raw: RawCheck) -> Vec<NetCheck> {
    let rules = raw.rules.map(OneOrMany::list).unwrap_or_default();
    let profiles = raw.profiles.map(OneOrMany::list).unwrap_or_default();
    raw.nets
        .map(OneOrMany::list)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| !private_adapter(&n.name))
        .map(|n| {
            let kind = if n.kind == "DomainAuthenticated" { "Domain".to_string() } else { n.kind };
            let firewall_on = profiles.iter().find(|p| p.name == kind).is_none_or(|p| p.enabled != "False");
            let blocked = rules.iter().any(|r| r.action == "Block" && covers(&r.profile, &kind));
            let allowed = !firewall_on || (!blocked && rules.iter().any(|r| r.action == "Allow" && covers(&r.profile, &kind)));
            NetCheck { name: n.name, kind, allowed }
        })
        .collect()
}

#[cfg(windows)]
fn encoded(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    B64.encode(utf16)
}

#[cfg(windows)]
fn quoted(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "''"))
}

#[cfg(windows)]
fn exe_quoted() -> Result<String, String> {
    Ok(quoted(&std::env::current_exe().map_err(err)?))
}

#[cfg(windows)]
async fn powershell(script: &str, wait: Duration) -> Result<std::process::Output, String> {
    let mut cmd = tokio::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded(script)]).kill_on_drop(true);
    cmd.creation_flags(0x0800_0000);
    timeout(wait, cmd.output()).await.map_err(|_| "Windows took too long to answer.".to_string())?.map_err(err)
}

async fn firewall() -> FirewallView {
    match std::env::current_exe() {
        Ok(exe) => firewall_for(&exe).await,
        Err(_) => FirewallView::default(),
    }
}

/// The check for one program.
async fn firewall_for(exe: &std::path::Path) -> FirewallView {
    #[cfg(windows)]
    {
        let exe = quoted(exe);
        let script = format!(
            "$exe = {exe}\n\
             $nets = @(Get-NetConnectionProfile | ForEach-Object {{ [pscustomobject]@{{ name = [string]$_.InterfaceAlias; kind = [string]$_.NetworkCategory }} }})\n\
             $rules = @(Get-NetFirewallApplicationFilter -Program $exe -ErrorAction SilentlyContinue | Get-NetFirewallRule -ErrorAction SilentlyContinue | Where-Object {{ [string]$_.Direction -eq 'Inbound' -and [string]$_.Enabled -eq 'True' }} | ForEach-Object {{ [pscustomobject]@{{ action = [string]$_.Action; profile = [string]$_.Profile }} }})\n\
             $profiles = @(Get-NetFirewallProfile | ForEach-Object {{ [pscustomobject]@{{ name = [string]$_.Name; enabled = [string]$_.Enabled }} }})\n\
             [pscustomobject]@{{ nets = $nets; rules = $rules; profiles = $profiles }} | ConvertTo-Json -Depth 4 -Compress"
        );
        let Ok(out) = powershell(&script, Duration::from_secs(20)).await else { return FirewallView::default() };
        match serde_json::from_slice::<RawCheck>(&out.stdout) {
            Ok(raw) => FirewallView { checked: true, networks: judge(raw) },
            Err(_) => FirewallView::default(),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = exe;
        FirewallView::default()
    }
}

/// Whether people can reach SulcusAI on each network this PC is on.
#[tauri::command]
pub async fn people_firewall() -> FirewallView {
    firewall().await
}

/// Removes any rule keeping SulcusAI from being reached, and allows it on
/// every network. Needs an administrator.
#[cfg(windows)]
fn fix_script(exe: &str) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $exe = {exe}\n\
         Get-NetFirewallApplicationFilter -Program $exe -ErrorAction SilentlyContinue | Get-NetFirewallRule -ErrorAction SilentlyContinue | Where-Object {{ [string]$_.Direction -eq 'Inbound' -and [string]$_.Action -eq 'Block' }} | Remove-NetFirewallRule\n\
         New-NetFirewallRule -DisplayName 'SulcusAI (People)' -Direction Inbound -Action Allow -Program $exe -Profile Any | Out-Null"
    )
}

/// Runs a script as administrator: Windows asks the user first. Exit code
/// 5: the user said no.
#[cfg(windows)]
fn elevated(script: &str) -> String {
    format!(
        "try {{ $p = Start-Process -FilePath powershell.exe -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-EncodedCommand','{}'; exit $p.ExitCode }} catch {{ exit 5 }}",
        encoded(script)
    )
}

/// Lets people reach SulcusAI on every network (removing any rule that
/// blocks it), after Windows' permission prompt.
#[tauri::command]
pub async fn people_allow_firewall(state: AppStateRef<'_>) -> Result<FirewallView, String> {
    #[cfg(windows)]
    {
        let exe = exe_quoted()?;
        let out = powershell(&elevated(&fix_script(&exe)), Duration::from_secs(180)).await?;
        if out.status.code() == Some(5) {
            return Err("Windows didn't get permission (the prompt was declined), so nothing changed.".into());
        }
        if !out.status.success() {
            return Err("Windows Firewall didn't take the change.".into());
        }
        state.log("security", "Allowed SulcusAI through Windows Firewall on every network (People)");
        Ok(firewall().await)
    }
    #[cfg(not(windows))]
    {
        let _ = state;
        Err("This is only needed on Windows.".into())
    }
}

/// A code for tests: (its id, the code).
#[cfg(test)]
pub(super) fn code_for_test(state: &AppState, max: u32) -> (String, String) {
    let c = state.cipher().unwrap();
    update(state, |b| {
        let taken: Vec<String> = b.invites.iter().filter(|i| i.open()).filter_map(|i| c.decrypt(&i.code).ok()).collect();
        let code = fresh_code(&taken);
        let id = uuid::Uuid::new_v4().simple().to_string();
        b.invites.push(Invite { id: id.clone(), code: c.encrypt(&code), max, created_at: db::now_ms(), ..Default::default() });
        (id, code)
    })
    .unwrap()
}

#[cfg(test)]
pub(super) fn invite(state: &AppState, id: &str) -> Invite {
    book(&state.db.lock().unwrap()).invites.into_iter().find(|i| i.id == id).unwrap()
}

#[cfg(test)]
pub(super) fn block_now(state: &AppState, id: &str, block: bool) {
    update(state, |b| {
        b.blocked.retain(|x| x.id != id);
        if block {
            b.blocked.push(Blocked { id: id.into(), name: "Ben".into(), at: 0 });
        }
    })
    .unwrap();
    if block {
        forget_contact(state, id).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_closes_when_used_up_turned_off_or_guessed_at() {
        let mut i = Invite { max: 2, ..Default::default() };
        assert!(i.open());
        i.used.push(Use::default());
        assert!(i.open(), "one of two");
        i.used.push(Use::default());
        assert_eq!(i.why_closed(), Some("Used up."));
        let mut j = Invite { max: 1, wrong: WRONG_LIMIT, ..Default::default() };
        assert!(j.why_closed().unwrap().contains("wrong tries"));
        j.wrong = 0;
        j.closed = true;
        assert_eq!(j.why_closed(), Some("Turned off."));
        // A new code never starts like an open one.
        let taken = vec!["K7Q29XMB".to_string()];
        for _ in 0..200 {
            assert!(!fresh_code(&taken).starts_with("K7"));
        }
    }

    #[test]
    fn the_firewall_check_reads_windows_answers() {
        let raw: RawCheck = serde_json::from_str(
            r#"{"nets":[{"name":"Wi-Fi","kind":"Private"},{"name":"Radmin VPN","kind":"Public"},{"name":"vEthernet (WSL)","kind":"Public"}],
                "rules":[{"action":"Allow","profile":"Private"}],
                "profiles":[{"name":"Private","enabled":"True"},{"name":"Public","enabled":"True"}]}"#,
        )
        .unwrap();
        let nets = judge(raw);
        assert_eq!(nets.len(), 2, "virtual machine adapters are left out");
        assert!(nets[0].allowed && !nets[1].allowed, "allowed at home, blocked on Radmin (Public)");
        // A single network, a block rule, and the firewall off for a profile.
        let raw: RawCheck = serde_json::from_str(r#"{"nets":{"name":"Radmin VPN","kind":"Public"},"rules":[{"action":"Allow","profile":"Any"},{"action":"Block","profile":"Public"}],"profiles":[]}"#).unwrap();
        assert!(!judge(raw)[0].allowed, "a block rule wins");
        let raw: RawCheck = serde_json::from_str(r#"{"nets":[{"name":"Radmin VPN","kind":"Public"}],"rules":[],"profiles":[{"name":"Public","enabled":"False"}]}"#).unwrap();
        assert!(judge(raw)[0].allowed, "firewall off");
    }

    /// The scripts the fix runs are valid PowerShell (parsed, not run:
    /// running needs an administrator), even with a quote in the path.
    #[cfg(windows)]
    #[tokio::test]
    async fn the_firewall_fix_is_valid_powershell() {
        let fix = fix_script(&quoted(std::path::Path::new(r"C:\Users\o'brien\AppData\Local\SulcusAI\sulcusai.exe")));
        for script in [fix.clone(), elevated(&fix)] {
            let check = format!(
                "$errors = $null; [void][System.Management.Automation.Language.Parser]::ParseInput([Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('{}')), [ref]$null, [ref]$errors); $errors.Count",
                encoded(&script)
            );
            let out = powershell(&check, Duration::from_secs(30)).await.unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "0", "{script}");
        }
    }

    /// Asks this PC's Windows Firewall: `cargo test --lib checks_this_pcs_firewall -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn checks_this_pcs_firewall() {
        eprintln!("this test: {:?}", firewall().await);
        let installed = std::path::Path::new(&std::env::var("LOCALAPPDATA").unwrap_or_default()).join("SulcusAI").join("sulcusai.exe");
        eprintln!("installed app: {:?}", firewall_for(&installed).await);
    }
}
