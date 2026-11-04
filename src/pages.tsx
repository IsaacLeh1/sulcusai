// SPDX-License-Identifier: AGPL-3.0-only
// The app's pages, each loaded the first time it's opened, and the feature
// each belongs to (hidden while it's off).
import { lazy } from "react";
import { type FeatureId } from "./api";
import type { View } from "./App";

export const ModelsView = lazy(() => import("./views/ModelsView").then((m) => ({ default: m.ModelsView })));
export const SettingsView = lazy(() => import("./views/SettingsView").then((m) => ({ default: m.SettingsView })));
export const ActivityView = lazy(() => import("./views/ActivityView").then((m) => ({ default: m.ActivityView })));
export const WebViewerView = lazy(() => import("./views/WebViewerView").then((m) => ({ default: m.WebViewerView })));
export const MemoryView = lazy(() => import("./views/MemoryView").then((m) => ({ default: m.MemoryView })));
export const ProjectView = lazy(() => import("./views/ProjectView").then((m) => ({ default: m.ProjectView })));
export const ScheduledView = lazy(() => import("./views/ScheduledView").then((m) => ({ default: m.ScheduledView })));
export const ConnectorsView = lazy(() => import("./views/ConnectorsView").then((m) => ({ default: m.ConnectorsView })));
export const MeetingsView = lazy(() => import("./views/MeetingsView").then((m) => ({ default: m.MeetingsView })));
export const TranslateView = lazy(() => import("./views/TranslateView").then((m) => ({ default: m.TranslateView })));
export const PersonalView = lazy(() => import("./views/PersonalView").then((m) => ({ default: m.PersonalView })));
export const FeaturesView = lazy(() => import("./views/FeaturesView").then((m) => ({ default: m.FeaturesView })));
export const NotesView = lazy(() => import("./views/NotesView").then((m) => ({ default: m.NotesView })));
export const DocumentsView = lazy(() => import("./views/DocumentsView").then((m) => ({ default: m.DocumentsView })));
export const StudyView = lazy(() => import("./views/StudyView").then((m) => ({ default: m.StudyView })));
export const ReviewView = lazy(() => import("./views/ReviewView").then((m) => ({ default: m.ReviewView })));
export const ComputeView = lazy(() => import("./views/ComputeView").then((m) => ({ default: m.ComputeView })));
export const TasksView = lazy(() => import("./views/TasksView").then((m) => ({ default: m.TasksView })));
export const MailView = lazy(() => import("./views/MailView").then((m) => ({ default: m.MailView })));
export const CalendarView = lazy(() => import("./views/CalendarView").then((m) => ({ default: m.CalendarView })));
export const StudioView = lazy(() => import("./views/StudioView").then((m) => ({ default: m.StudioView })));
export const PeopleView = lazy(() => import("./views/PeopleView").then((m) => ({ default: m.PeopleView })));
export const SharedChatView = lazy(() => import("./views/PeopleView").then((m) => ({ default: m.SharedChatView })));
export const Onboarding = lazy(() => import("./views/Onboarding").then((m) => ({ default: m.Onboarding })));
export const LibraryView = lazy(() => import("./views/LibraryView").then((m) => ({ default: m.LibraryView })));
export const PcView = lazy(() => import("./views/PcView").then((m) => ({ default: m.PcView })));
export const WritingView = lazy(() => import("./views/WritingView").then((m) => ({ default: m.WritingView })));

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
  project: "projects",
};
