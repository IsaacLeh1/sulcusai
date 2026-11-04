import { describe, expect, it } from "vitest";
import type { Cadence, ScheduleView } from "../api/work";
import { workflows } from "./WorkflowEditor";

const task = (id: string, cadence: Cadence): ScheduleView => ({ id, name: id, prompt: "", cadence, model_id: null, project_id: null, allow_changes: false, actions: [], enabled: true, last_run: null, next_run: null, last_chat: null, when: "" }) as unknown as ScheduleView;

describe("workflows", () => {
  it("draws chains from their first task and skips lone tasks", () => {
    const items = [
      task("a", { kind: "on_start" }),
      task("b", { kind: "after_task", task: "a" }),
      task("c", { kind: "after_task", task: "b" }),
      task("d", { kind: "after_task", task: "a" }),
      task("lone", { kind: "hourly", minute: 0 }),
    ];
    const f = workflows(items);
    expect(f).toHaveLength(1);
    expect(f[0].task.id).toBe("a");
    expect(f[0].next.map((n) => n.task.id)).toEqual(["b", "d"]);
    expect(f[0].next[0].next[0].task.id).toBe("c");
  });
  it("survives a loop", () => {
    const items = [task("x", { kind: "on_start" }), task("y", { kind: "after_task", task: "z" }), task("z", { kind: "after_task", task: "y" })];
    expect(workflows(items)).toHaveLength(0);
  });
});
