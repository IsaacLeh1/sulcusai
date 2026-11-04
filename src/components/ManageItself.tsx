import { useEffect, useState } from "react";
import { api, errorText, type Chat, type ManagerPlan, type ManagerSettings } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";

const STATE_WORDS: Record<string, string> = { waiting: "Waiting", working: "Working", done: "Done, checked", stuck: "Stuck" };

/** The per-chat "Manage itself" switch, and its plan window. */
export function ManageItself({ chatId, onError }: { chatId: string; onError: (text: string) => void }) {
  const [settings, setSettings] = useState<ManagerSettings | null>(null);
  const [planOpen, setPlanOpen] = useState(false);
  useEffect(() => {
    let live = true;
    api.managerGet(chatId).then((s) => live && setSettings(s), () => live && setSettings(null));
    return () => {
      live = false;
    };
  }, [chatId]);
  if (!settings) return null;
  const change = async (next: ManagerSettings) => {
    try {
      await api.managerSet(chatId, next);
      setSettings(next);
    } catch (e) {
      onError(errorText(e));
    }
  };
  return (
    <>
      <button
        className={`globe ${settings.on ? "on" : ""}`}
        onClick={() => change({ ...settings, on: !settings.on })}
        aria-pressed={settings.on}
        title={t("Manage itself: a plan with checks, a budget per job, and a stop when the same step keeps failing")}
      >
        {settings.on ? t("Managing itself") : t("Manage itself")}
      </button>
      {settings.on && (
        <button className="globe" onClick={() => setPlanOpen(true)} title={t("The plan, its checks and the decisions made")}>
          {t("Plan")}
        </button>
      )}
      {planOpen && <PlanWindow chatId={chatId} settings={settings} onChange={change} onClose={() => setPlanOpen(false)} />}
    </>
  );
}

/** Read only when open, so a closed window costs nothing. */
function PlanWindow({ chatId, settings, onChange, onClose }: { chatId: string; settings: ManagerSettings; onChange: (s: ManagerSettings) => void; onClose: () => void }) {
  const [plan, setPlan] = useState<ManagerPlan | null>(null);
  const [chats, setChats] = useState<Chat[]>([]);
  const [answer, setAnswer] = useState("");
  const [note, setNote] = useState("");
  useEffect(() => {
    api.managerPlan(chatId).then(setPlan, () => setPlan(null));
    api.chats().then((c) => setChats(c.filter((x) => x.id !== chatId)), () => setChats([]));
  }, [chatId]);
  const who = settings.manager.kind === "itself" ? "itself" : settings.manager.kind === "chat" ? `chat:${settings.manager.id}` : "model";
  return (
    <Modal title={t("Plan")} onClose={onClose}>
      {!plan?.plan ? (
        <p>{t("No plan yet. It writes one at the start of the next job.")}</p>
      ) : (
        <ol>
          {plan.plan.steps.map((s, i) => (
            <li key={i}>
              {s.text} <em>({t(STATE_WORDS[s.state] ?? s.state)})</em>
            </li>
          ))}
        </ol>
      )}
      {plan && plan.decisions.length > 0 && (
        <>
          <h4>{t("Decisions")}</h4>
          <ul>
            {plan.decisions.map((d, i) => (
              <li key={i}>
                {d} <Redo chatId={chatId} decision={d} onError={setNote} onDone={() => setNote(t("It will redo this your way next turn."))} />
              </li>
            ))}
          </ul>
        </>
      )}
      {plan?.plan && (
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            try {
              const kept = await api.managerAnswer(chatId, answer);
              setAnswer("");
              setNote(t("Sent as: {text}", { text: kept }));
            } catch (err) {
              setNote(errorText(err));
            }
          }}
        >
          <textarea
            rows={2}
            aria-label={t("Answer the plan")}
            placeholder={t("Answer in two lines: approve, adjust, or a different effort or model")}
            value={answer}
            onChange={(e) => setAnswer(e.target.value)}
          />
          <button type="submit" disabled={!answer.trim()}>
            {t("Answer")}
          </button>
        </form>
      )}
      {note && <p role="status">{note}</p>}
      <label>
        {t("Managed by")}{" "}
        <select
          value={who}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "itself") onChange({ ...settings, manager: { kind: "itself" } });
            else if (v.startsWith("chat:")) onChange({ ...settings, manager: { kind: "chat", id: v.slice(5) } });
            else onChange({ ...settings, manager: { kind: "model", id: settings.manager.kind === "model" ? settings.manager.id : "" } });
          }}
        >
          <option value="itself">{t("Itself")}</option>
          {chats.map((c) => (
            <option key={c.id} value={`chat:${c.id}`}>
              {t("Reports to {title}", { title: c.title })}
            </option>
          ))}
          <option value="model">{t("A stronger model when it's stuck")}</option>
        </select>
      </label>
      {settings.manager.kind === "model" && (
        <input
          aria-label={t("Model to hand stuck steps to")}
          placeholder={t("Cloud model, as cloud:provider:model")}
          defaultValue={settings.manager.id}
          onBlur={(e) => onChange({ ...settings, manager: { kind: "model", id: e.target.value.trim() } })}
        />
      )}
    </Modal>
  );
}

/** "Redo this differently": the person's choice goes back to the chat for its next turn. */
function Redo({ chatId, decision, onError, onDone }: { chatId: string; decision: string; onError: (t: string) => void; onDone: () => void }) {
  const [open, setOpen] = useState(false);
  const [choice, setChoice] = useState("");
  if (!open)
    return (
      <button className="link" onClick={() => setOpen(true)}>
        {t("Redo this differently")}
      </button>
    );
  return (
    <form
      onSubmit={async (e) => {
        e.preventDefault();
        try {
          await api.managerRedo(chatId, decision, choice);
          setOpen(false);
          onDone();
        } catch (err) {
          onError(errorText(err));
        }
      }}
    >
      <input autoFocus aria-label={t("What to do instead")} placeholder={t("What to do instead")} value={choice} onChange={(e) => setChoice(e.target.value)} />
      <button type="submit" disabled={!choice.trim()}>
        {t("Redo")}
      </button>
    </form>
  );
}
