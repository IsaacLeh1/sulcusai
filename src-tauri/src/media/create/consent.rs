// SPDX-License-Identifier: AGPL-3.0-only
//! Consent for real people's faces and voices. The likeness notice covers
//! the person using the app; each job also says whose face or voice it uses,
//! so a job about someone else can't start without that person's yes.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Likeness {
    /// Made-up people, drawings, no one real.
    Nobody,
    /// The person at this PC.
    Myself,
    /// Someone else, real.
    Someone,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Claim {
    pub face: Option<Likeness>,
    pub voice: Option<Likeness>,
    /// Set by the window only after the person ticks "I have their permission".
    #[serde(default)]
    pub permission_from_them: bool,
    /// Public figures may be shown, never put words in their mouth (voice).
    #[serde(default)]
    pub public_figure: bool,
}

/// Whether the job may run. `notice_ok` is the likeness notice's state.
pub fn check(c: &Claim, notice_ok: bool) -> Result<(), String> {
    let uses = |l: Option<Likeness>| matches!(l, Some(Likeness::Myself | Likeness::Someone));
    if (uses(c.face) || uses(c.voice)) && !notice_ok {
        return Err(format!("{}likeness", crate::consent::NEEDED_PREFIX));
    }
    let other = |l: Option<Likeness>| l == Some(Likeness::Someone);
    if c.public_figure && other(c.voice) {
        return Err("A real public figure's voice can't be made to say new things.".into());
    }
    if (other(c.face) || other(c.voice)) && !c.permission_from_them && !c.public_figure {
        return Err("This uses a real person's face or voice. Confirm you have their permission first.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nobody_needs_nothing() {
        assert!(check(&Claim { face: Some(Likeness::Nobody), ..Default::default() }, false).is_ok());
    }
    #[test]
    fn myself_needs_notice() {
        let c = Claim { voice: Some(Likeness::Myself), ..Default::default() };
        assert!(check(&c, false).unwrap_err().starts_with("consent:"));
        assert!(check(&c, true).is_ok());
    }
    #[test]
    fn someone_needs_their_permission() {
        let mut c = Claim { face: Some(Likeness::Someone), ..Default::default() };
        assert!(check(&c, true).is_err());
        c.permission_from_them = true;
        assert!(check(&c, true).is_ok());
    }
    #[test]
    fn public_figure_voice_refused() {
        let c = Claim { voice: Some(Likeness::Someone), public_figure: true, permission_from_them: true, ..Default::default() };
        assert!(check(&c, true).is_err());
        let f = Claim { face: Some(Likeness::Someone), public_figure: true, ..Default::default() };
        assert!(check(&f, true).is_ok());
    }
}
