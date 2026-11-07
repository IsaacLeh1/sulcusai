// SPDX-License-Identifier: AGPL-3.0-only
// The app's pages, each loaded the first time it's opened, and the feature
// each belongs to (hidden while it's off).
import { lazy, type ComponentType } from "react";
import { type FeatureId } from "./api";
import type { View } from "./App";

/** Loads a view with retries when network adapter changes interrupt local dev sockets. */
function retryView<T extends Record<string, any>, K extends keyof T>(load: () => Promise<T>, key: K) {
  return lazy(() => {
    const tryLoad = (retries = 3, delay = 600): Promise<{ default: ComponentType<any> }> =>
      load()
        .then((m) => ({ default: m[key] }))
        .catch((err) => {
          if (retries <= 0) throw err;
          return new Promise((resolve) => setTimeout(resolve, delay)).then(() => tryLoad(retries - 1, delay * 1.5));
        });
    return tryLoad();
  }) as unknown as T[K];
}

export const ModelsView = retryView(() => import("./views/ModelsView"), "ModelsView");
export const SettingsView = retryView(() => import("./views/SettingsView"), "SettingsView");
export const ActivityView = retryView(() => import("./views/ActivityView"), "ActivityView");
export const WebViewerView = retryView(() => import("./views/WebViewerView"), "WebViewerView");
export const MemoryView = retryView(() => import("./views/MemoryView"), "MemoryView");
export const ProjectView = retryView(() => import("./views/ProjectView"), "ProjectView");
export const ScheduledView = retryView(() => import("./views/ScheduledView"), "ScheduledView");
export const ConnectorsView = retryView(() => import("./views/ConnectorsView"), "ConnectorsView");
export const MeetingsView = retryView(() => import("./views/MeetingsView"), "MeetingsView");
export const TranslateView = retryView(() => import("./views/TranslateView"), "TranslateView");
export const PersonalView = retryView(() => import("./views/PersonalView"), "PersonalView");
export const FeaturesView = retryView(() => import("./views/FeaturesView"), "FeaturesView");
export const NotesView = retryView(() => import("./views/NotesView"), "NotesView");
export const DocumentsView = retryView(() => import("./views/DocumentsView"), "DocumentsView");
export const StudyView = retryView(() => import("./views/StudyView"), "StudyView");
export const ReviewView = retryView(() => import("./views/ReviewView"), "ReviewView");
export const ComputeView = retryView(() => import("./views/ComputeView"), "ComputeView");
export const TasksView = retryView(() => import("./views/TasksView"), "TasksView");
export const MailView = retryView(() => import("./views/MailView"), "MailView");
export const CalendarView = retryView(() => import("./views/CalendarView"), "CalendarView");
export const StudioView = retryView(() => import("./views/StudioView"), "StudioView");
export const PeopleView = retryView(() => import("./views/PeopleView"), "PeopleView");
export const NetworksView = retryView(() => import("./views/NetworksView"), "NetworksView");
export const SharedChatView = retryView(() => import("./views/PeopleView"), "SharedChatView");
export const Onboarding = retryView(() => import("./views/Onboarding"), "Onboarding");
export const LibraryView = retryView(() => import("./views/LibraryView"), "LibraryView");
export const PcView = retryView(() => import("./views/PcView"), "PcView");
export const WritingView = retryView(() => import("./views/WritingView"), "WritingView");

/** Pages that belong to a feature, hidden while it's off. */
export const VIEW_FEATURE: Partial<Record<View, FeatureId>> = {
  meetings: "meetings",
  translate: "translate",
  notes: "notes",
  library: "library",
  documents: "documents",
  study: "documents",
  review: "files",
  tasks: "tasks",
  mail: "email",
  calendar: "calendar",
  memory: "memory",
  scheduled: "scheduled",
  connectors: "connectors",
  people: "people",
  networks: "people",
  project: "projects",
};
export const HomeView = retryView(() => import("./views/HomeView"), "HomeView");
export const NetworksPage = retryView(() => import("./views/NetworksPage"), "NetworksPage");
