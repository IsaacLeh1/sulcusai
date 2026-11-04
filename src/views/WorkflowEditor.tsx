import type { Schedule, ScheduleView } from "../api/work";
import { t } from "../i18n";

export interface Flow {
  task: ScheduleView;
  next: Flow[];
}

/** Tasks joined by "Right after another task", as trees from the task that starts each.
 *  A loop (which the runner refuses) is cut where it closes. */
export function workflows(items: ScheduleView[]): Flow[] {
  const after = (s: Schedule) => (s.cadence.kind === "after_task" ? s.cadence.task : null);
  const ids = new Set(items.map((s) => s.id));
  const grow = (s: ScheduleView, seen: Set<string>): Flow => {
    const path = new Set(seen).add(s.id);
    return { task: s, next: items.filter((x) => after(x) === s.id && !path.has(x.id)).map((x) => grow(x, path)) };
  };
  return items
    .filter((s) => {
      const up = after(s);
      return !up || !ids.has(up);
    })
    .map((s) => grow(s, new Set()))
    .filter((f) => f.next.length > 0);
}

/** The visual workflow editor: each chain drawn as steps with arrows; a
 *  step opens in the usual editor, and a step can be added after any one. */
export function WorkflowEditor({ items, onEdit, onAddAfter }: { items: ScheduleView[]; onEdit: (s: ScheduleView) => void; onAddAfter: (id: string) => void }) {
  const flows = workflows(items);
  const starts = items.filter((s) => s.cadence.kind !== "after_task" && !flows.some((f) => f.task.id === s.id));
  return (
    <section className="card workflows">
      <h2>{t("Workflows")}</h2>
      <p className="small muted">{t("Steps run one after another, each handed the answer of the one before. A step runs only when the one before finished without an error.")}</p>
      {flows.length === 0 && <p className="muted">{t("No workflows yet. Add a step after any task to start one.")}</p>}
      {flows.map((f) => (
        <Steps key={f.task.id} flow={f} first onEdit={onEdit} onAddAfter={onAddAfter} />
      ))}
      {starts.length > 0 && (
        <label className="small">
          {t("Start a workflow from")}{" "}
          <select value="" onChange={(e) => e.target.value && onAddAfter(e.target.value)}>
            <option value="">{t("Pick a task")}</option>
            {starts.map((s) => (
              <option key={s.id} value={s.id}>
                {s.name}
              </option>
            ))}
          </select>
        </label>
      )}
    </section>
  );
}

function Steps({ flow, first, onEdit, onAddAfter }: { flow: Flow; first?: boolean; onEdit: (s: ScheduleView) => void; onAddAfter: (id: string) => void }) {
  const s = flow.task;
  return (
    <div className="flow" style={{ marginLeft: first ? 0 : 24 }}>
      <div className="row flow-step">
        {!first && <span aria-hidden="true">&rarr;</span>}
        <button className="btn ghost small" onClick={() => onEdit(s)} title={s.when}>
          {s.name}
        </button>
        {first && <span className="small muted">{s.when}</span>}
        {!s.enabled && <span className="tag">{t("Paused")}</span>}
        {s.last_error && <span className="tag warn">{t("Last run failed")}</span>}
        <button className="link small" onClick={() => onAddAfter(s.id)}>
          {t("+ Step after")}
        </button>
      </div>
      {flow.next.map((n) => (
        <Steps key={n.task.id} flow={n} onEdit={onEdit} onAddAfter={onAddAfter} />
      ))}
    </div>
  );
}
