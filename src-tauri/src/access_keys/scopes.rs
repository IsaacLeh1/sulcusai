// SPDX-License-Identifier: AGPL-3.0-only
//! What an access key may be allowed to do, one scope per kind of thing, in
//! words a consent screen can show as they are. A key starts with none.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    ReadChats,
    WriteChats,
    ReadMail,
    SendMail,
    ReadCalendar,
    WriteCalendar,
    /// Limited to the folders chosen for the key.
    ReadFiles,
    /// Limited to the folders chosen for the key.
    WriteFiles,
    RunTools,
    UseModels,
    /// Kept apart from `UseModels` because cloud models cost money.
    UseCloudModels,
    /// The assistant works with whatever the app allows it, so this one
    /// reaches further than its name suggests; the description says so.
    UseAssistant,
    ReadMemories,
    WriteMemories,
}

/// How much harm a careless or hostile app could do with a scope, so a
/// consent screen can make the risky ones stand out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScopeInfo {
    pub scope: Scope,
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub risk: Risk,
}

impl Scope {
    pub const ALL: [Scope; 14] = [
        Scope::ReadChats,
        Scope::WriteChats,
        Scope::ReadMail,
        Scope::SendMail,
        Scope::ReadCalendar,
        Scope::WriteCalendar,
        Scope::ReadFiles,
        Scope::WriteFiles,
        Scope::RunTools,
        Scope::UseModels,
        Scope::UseCloudModels,
        Scope::UseAssistant,
        Scope::ReadMemories,
        Scope::WriteMemories,
    ];

    pub fn info(self) -> ScopeInfo {
        use Risk::*;
        let (id, title, description, risk) = match self {
            Scope::ReadChats => ("read_chats", "Read your chats", "See your chats and everything said in them, including pictures and files you added.", Medium),
            Scope::WriteChats => ("write_chats", "Change your chats", "Start chats, add messages, rename chats and move them between folders.", Medium),
            Scope::ReadMail => ("read_mail", "Read your mail", "Read the mail in the accounts you connected to SulcusAI, with senders and attachments.", Medium),
            Scope::SendMail => ("send_mail", "Send mail as you", "Send, reply to and forward mail from your connected accounts. People will see it as coming from you.", High),
            Scope::ReadCalendar => ("read_calendar", "See your calendar", "See your events, who is invited and where they are.", Medium),
            Scope::WriteCalendar => ("write_calendar", "Change your calendar", "Add, move and delete events, and send invitations from your calendar.", High),
            Scope::ReadFiles => ("read_files", "Read files in chosen folders", "Open and read files, but only inside the folders you choose for this app.", Medium),
            Scope::WriteFiles => ("write_files", "Change files in chosen folders", "Create, change and delete files, but only inside the folders you choose for this app.", High),
            Scope::RunTools => ("run_tools", "Run tools and commands", "Run the assistant's tools and commands on this PC. A command can do almost anything your account can.", High),
            Scope::UseModels => ("use_models", "Use the models on this PC", "Send text and pictures to the AI models on this PC and get their answers. Nothing leaves this PC.", Low),
            Scope::UseCloudModels => ("use_cloud_models", "Use your cloud models", "Use the cloud models you signed in to. What it sends goes to that provider and may cost you money.", High),
            Scope::UseAssistant => ("use_assistant", "Use the SulcusAI assistant", "Ask the assistant to do things for you. It can use whatever you let it use in SulcusAI, such as your folders, accounts and the web.", High),
            Scope::ReadMemories => ("read_memories", "Read what SulcusAI remembers", "See the things SulcusAI remembers about you from your chats.", Medium),
            Scope::WriteMemories => ("write_memories", "Change what SulcusAI remembers", "Add, change and forget the things SulcusAI remembers about you.", Medium),
        };
        ScopeInfo { scope: self, id, title, description, risk }
    }

    pub fn id(self) -> &'static str {
        self.info().id
    }

    pub fn parse(id: &str) -> Option<Scope> {
        let id = id.trim();
        Scope::ALL.into_iter().find(|s| s.id().eq_ignore_ascii_case(id))
    }

    /// File scopes are only ever granted for a path inside the key's folders.
    pub fn is_files(self) -> bool {
        matches!(self, Scope::ReadFiles | Scope::WriteFiles)
    }
}

/// Every scope, for a settings page or a consent screen.
pub fn catalog() -> Vec<ScopeInfo> {
    Scope::ALL.into_iter().map(Scope::info).collect()
}
