// SPDX-License-Identifier: AGPL-3.0-only
import type { Chat } from "../api";
import { WebPermission } from "../components/WebPermission";
import { FolderPermission } from "../components/FolderPermission";
import { KeyPermission } from "../components/KeyPermission";
import { SharePermission } from "../components/SharePermission";
import { MemoryWarningPopup } from "../components/MemoryWarning";
import { CardAskPopup } from "../components/CardAsk";
import { DriverNotice } from "../components/DriverNotice";
import { ElsewhereApprovals } from "../components/ElsewhereApprovals";
import { CrashReportPrompt } from "../components/Feedback";
import { PeopleRequests } from "../components/PeopleRequests";
import type { PushToast } from "../components/Toasts";

/** Everything that asks for the user's attention over any page: permissions, warnings, approvals, requests. */
export function Popups({ chats, openChatId, onOpenChat, peopleOn, onOpenPeople, toast }: {
  chats: Chat[];
  /** The chat on screen now, or null on another page. */
  openChatId: string | null;
  onOpenChat: (id: string) => void;
  peopleOn: boolean;
  onOpenPeople: () => void;
  toast: PushToast;
}) {
  return (
    <>
      <WebPermission chats={chats} onOpenChat={onOpenChat} />
      <FolderPermission chats={chats} onOpenChat={onOpenChat} />
      <KeyPermission chats={chats} onOpenChat={onOpenChat} />
      <SharePermission chats={chats} onOpenChat={onOpenChat} />
      <MemoryWarningPopup />
      <CardAskPopup />
      <DriverNotice />
      <ElsewhereApprovals chats={chats} openChatId={openChatId} onOpenChat={onOpenChat} />
      <CrashReportPrompt toast={toast} />
      <PeopleRequests enabled={peopleOn} onOpenPeople={onOpenPeople} toast={toast} />
    </>
  );
}
