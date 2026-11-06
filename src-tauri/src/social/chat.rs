// SPDX-License-Identifier: AGPL-3.0-only
//! Messages in a group: posts, replies, threads, reactions, pins, edits and
//! deletes. Each event travels sealed with the group key; this module keeps
//! the opened view and checks every event against the group's rules.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use super::groups::{ChannelKind, Group, GroupKey, Role};
use super::MemberId;

pub const MAX_TEXT: usize = 4000;
pub const MAX_REACTIONS: usize = 20;
pub const MAX_PINS: usize = 50;
/// Mentions in one message; more reads as a mention flood.
pub const MAX_MENTIONS: usize = 20;
/// Base64 characters of files in one message (about 3 MB): every member's
/// PC stores and relays them, so larger files go through shared folders.
pub const MAX_FILE_CHARS: usize = 4_000_000;
pub const MAX_FILES: usize = 10;

/// A picture, video, sound, voice message or other file sent in a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileRef {
    pub name: String,
    pub mime: String,
    /// Base64 of the bytes.
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: String,
    pub channel: String,
    pub author: MemberId,
    pub text: String,
    pub at: i64,
    pub reply_to: Option<String>,
    /// The message a thread hangs from.
    pub thread: Option<String>,
    pub edited: bool,
    pub deleted: bool,
    pub reactions: BTreeMap<String, BTreeSet<MemberId>>,
    pub mentions: Vec<MemberId>,
    #[serde(default)]
    pub files: Vec<FileRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum Event {
    Post {
        id: String,
        channel: String,
        text: String,
        reply_to: Option<String>,
        thread: Option<String>,
        mentions: Vec<MemberId>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<FileRef>,
    },
    /// Someone is typing; shown for a few seconds and never stored.
    Typing { channel: String },
    /// Someone read a channel up to `at`, for read marks.
    Read { channel: String, at: i64 },
    Edit { id: String, text: String },
    Delete { id: String },
    React { id: String, reaction: String, on: bool },
    Pin { id: String, on: bool },
}

/// One event as it travels: who sent it and when, sealed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub group: String,
    pub channel: String,
    pub from: MemberId,
    pub at: i64,
    pub blob: Vec<u8>,
}

fn bound(group: &str, channel: &str, from: &str, at: i64) -> String {
    format!("{group}|{channel}|{from}|{at}")
}

pub fn seal(key: &GroupKey, group: &str, channel: &str, from: &str, at: i64, ev: &Event) -> Envelope {
    let plain = serde_json::to_vec(ev).expect("event json");
    Envelope { group: group.into(), channel: channel.into(), from: from.into(), at, blob: key.seal(&bound(group, channel, from, at), &plain) }
}

pub fn open(key: &GroupKey, env: &Envelope) -> Result<Event, String> {
    let plain = key.open(&bound(&env.group, &env.channel, &env.from, env.at), &env.blob)?;
    let ev: Event = serde_json::from_slice(&plain).map_err(|_| "A group message didn't make sense.".to_string())?;
    // The sealed channel must match the one on the outside.
    if let Event::Post { channel, .. } = &ev {
        if *channel != env.channel {
            return Err("A group message named the wrong channel.".into());
        }
    }
    Ok(ev)
}

/// The opened messages of one group, as this PC sees them.
#[derive(Debug, Default, Clone)]
pub struct Room {
    pub messages: Vec<Message>,
    pub pins: BTreeMap<String, Vec<String>>,
    last_post: HashMap<(String, MemberId), i64>,
    /// Per channel, how far each member has read.
    pub seen: BTreeMap<String, BTreeMap<MemberId, i64>>,
}

fn check_files(files: &[FileRef]) -> Result<(), String> {
    if files.len() > MAX_FILES {
        return Err("Ten files at most in one message.".into());
    }
    if files.iter().map(|f| f.data.len()).sum::<usize>() > MAX_FILE_CHARS {
        return Err("Files in one message are 3 MB at most. Share bigger ones through a shared folder.".into());
    }
    for f in files {
        if f.name.is_empty() || f.name.chars().count() > 200 || f.mime.len() > 100 || f.mime.contains(['<', '>', '"']) {
            return Err("A file in that message isn't valid.".into());
        }
    }
    Ok(())
}

impl Room {
    pub fn find(&self, id: &str) -> Option<&Message> {
        self.messages.iter().find(|m| m.id == id)
    }

    fn find_mut(&mut self, id: &str) -> Result<&mut Message, String> {
        self.messages.iter_mut().find(|m| m.id == id).ok_or_else(|| "That message isn't here.".to_string())
    }

    /// Applies an event from `from` at `at`, checked against the group.
    pub fn apply(&mut self, g: &Group, from: &str, at: i64, ev: Event) -> Result<(), String> {
        let me = g.members.get(from).ok_or("They aren't in this group.")?;
        let moderator = me.role >= Role::Moderator;
        match ev {
            Event::Post { id, channel, text, reply_to, thread, mentions, files } => {
                let ch = g.channel(&channel).ok_or("No such channel.")?;
                if ch.kind == ChannelKind::Voice {
                    return Err("Voice channels don't take messages.".into());
                }
                if ch.kind == ChannelKind::Announcements && !moderator {
                    return Err("Only moderators post here.".into());
                }
                if ch.adults_only && me.teen {
                    return Err("This channel isn't open to teen members.".into());
                }
                if me.muted_until > at {
                    return Err("You're muted in this group for now.".into());
                }
                if (text.trim().is_empty() && files.is_empty()) || text.chars().count() > MAX_TEXT {
                    return Err("Messages are 1 to 4000 characters.".into());
                }
                check_files(&files)?;
                if mentions.len() > MAX_MENTIONS {
                    return Err("Too many mentions in one message.".into());
                }
                if ch.slow_secs > 0 && !moderator {
                    if let Some(last) = self.last_post.get(&(channel.clone(), from.to_string())) {
                        if at - last < ch.slow_secs as i64 * 1000 {
                            return Err("Slow mode is on in this channel.".into());
                        }
                    }
                }
                if self.find(&id).is_some() {
                    return Err("That message is already here.".into());
                }
                for parent in reply_to.iter().chain(thread.iter()) {
                    if self.find(parent).map(|p| p.channel != channel).unwrap_or(true) {
                        return Err("A reply must be in the same channel.".into());
                    }
                }
                self.last_post.insert((channel.clone(), from.to_string()), at);
                self.messages.push(Message { id, channel, author: from.into(), text, at, reply_to, thread, edited: false, deleted: false, reactions: BTreeMap::new(), mentions, files });
            }
            Event::Edit { id, text } => {
                if text.trim().is_empty() || text.chars().count() > MAX_TEXT {
                    return Err("Messages are 1 to 4000 characters.".into());
                }
                let m = self.find_mut(&id)?;
                if m.author != from || m.deleted {
                    return Err("Only the author can edit a message.".into());
                }
                m.text = text;
                m.edited = true;
            }
            Event::Delete { id } => {
                let m = self.find_mut(&id)?;
                if m.author != from && !moderator {
                    return Err("Only the author or a moderator can delete this.".into());
                }
                m.deleted = true;
                m.text.clear();
                m.files.clear();
                m.reactions.clear();
            }
            Event::React { id, reaction, on } => {
                if reaction.is_empty() || reaction.chars().count() > 32 {
                    return Err("That reaction isn't valid.".into());
                }
                let m = self.find_mut(&id)?;
                if m.deleted {
                    return Err("That message was deleted.".into());
                }
                if on && !m.reactions.contains_key(&reaction) && m.reactions.len() >= MAX_REACTIONS {
                    return Err("That message has as many reactions as it can take.".into());
                }
                let set = m.reactions.entry(reaction.clone()).or_default();
                if on {
                    set.insert(from.into());
                } else {
                    set.remove(from);
                    if set.is_empty() {
                        m.reactions.remove(&reaction);
                    }
                }
            }
            Event::Pin { id, on } => {
                if !moderator {
                    return Err("Only moderators pin messages.".into());
                }
                let channel = self.find(&id).ok_or("That message isn't here.")?.channel.clone();
                let pins = self.pins.entry(channel).or_default();
                pins.retain(|p| *p != id);
                if on {
                    if pins.len() >= MAX_PINS {
                        return Err("This channel has as many pins as it can take.".into());
                    }
                    pins.push(id);
                }
            }
            Event::Typing { .. } => {}
            Event::Read { channel, at: upto } => {
                let seen = self.seen.entry(channel).or_default();
                let best = seen.entry(from.to_string()).or_default();
                *best = (*best).max(upto.min(at));
            }
        }
        Ok(())
    }

    /// A channel's top-level messages, oldest first (threads open apart).
    pub fn channel(&self, channel: &str) -> Vec<&Message> {
        self.messages.iter().filter(|m| m.channel == channel && m.thread.is_none()).collect()
    }

    pub fn thread(&self, root: &str) -> Vec<&Message> {
        self.messages.iter().filter(|m| m.thread.as_deref() == Some(root)).collect()
    }

    /// Messages after `since` that mention `who`.
    pub fn mentions_of(&self, who: &str, since: i64) -> Vec<&Message> {
        self.messages.iter().filter(|m| m.at > since && !m.deleted && m.mentions.iter().any(|x| x == who)).collect()
    }
}
