import type { AgentSessionEventWebhookPayload } from "@linear/sdk/webhooks";
import type { ActivityInput } from "../shared/linear.ts";
import { phrase } from "../shared/voice.ts";

// The Worker never does the work. It only answers inside Linear's ten-second window so
// the session is not marked unresponsive, and hands the owner's session or prompt to the
// Durable Object's state machine as an event (events.ts, `fromSession`), which decides.
//
// Nothing reads these messages back, so they can be reworded freely.
//
// The acknowledgements of a delegation ("acknowledging"), of a mention or reply
// ("heard"), and of a stop ("stopping") are phrases from src/shared/voice.json, posted
// as ephemeral thoughts, so what `tolkster listen` posts next replaces them. A durable
// one stayed on the session's card as its main text long after the work had started.
// When nothing is listening, though, nothing replaces them, and the card says Thinking
// for as long as the workstation is off: then the acknowledgement is AWAY, a response,
// so the card says so instead (`acknowledgement`). When `listen` goes quiet after an
// ephemeral one, the request's `quiet` deadline says AWAY later (`postAway`, which the
// relay posts).

export const NOT_OWNER = "I only take work from my owner. Nothing personal.";

// The acknowledgement when nothing is listening: a response, so it stays as the card's
// main text until Tolkster's next response: progress and most pickups are thoughts, and
// only a start after a waiting notice is a response. It promises no more than a later
// start, which the machine keeps: its commands wait for `listen`. Owl, no plumbing.
export const AWAY = "I'm off my perch. I'll pick this up when I'm back.";

// A mention somewhere other than an issue, e.g. a document or a project. `tolkster
// listen` never picks these up, so this is a response: nothing else will follow it.
export const NO_ISSUE = "I only work on issues, for now.";

// A session of Tolkster's own chain whose chain no longer holds: the machine made no request
// from it (relay.ts, `ingest`), and this is the only reply it gets. A response, so it ends the
// session.
export const NOT_HELD = "That delegation isn't one I hold, so I took no work from it. Delegate it to me again if you want it done.";

export type Decision = {
  // What was decided, for the log.
  kind: "start" | "deliver" | "stop" | "not_owner" | "no_issue";
  activity: ActivityInput;
  // Set when someone other than the owner delegated the issue, never for their mention.
  // Taking the delegation back keeps it out of Tolkster's list of delegated issues. It
  // names the session's creator and the app: the delegation to the app is cleared only if
  // the issue's history says the creator made the current one (`clearDelegateMadeBy`).
  // The undelegation makes no event, as anyone's but the owner's makes none.
  undelegate?: { issueId: string; creatorId: string; appUserId: string };
  // The owner's request, for the machine. Only for what the owner started or wrote, or a
  // chain's delegation; never for a refusal.
  request?: SessionRequest;
};

// Why `decide` trusted the session: it is the owner's, the owner's reply in Tolkster's own
// thread, or a delegation's that Tolkster's chain made. The one place it is worked out.
export type SessionTrust = "owner" | "reply" | "chain";

// What the machine's event takes from the session. `at` is Linear's time for the request:
// the session's `createdAt`, or the prompt's. `commentId` is the owner's comment that
// mentioned Tolkster, by its ID, for the owner's mention's session; a reply in Tolkster's own
// thread names Tolkster's comment, so it has none. The shapes of the IDs and the time are
// checked where the event is made (events.ts, `fromSession`).
export type SessionRequest = { trust: SessionTrust; sessionKind: "mention" | "delegation"; commentId: string | null; identifier: string | null; at: string };

// `random` picks the acknowledgement's phrase; a test can fix it. `ownDelegation` says the
// session's issue is a sub-issue Tolkster delegated to itself (state.ts, `Delegation`): its
// delegation's session is taken like the owner's, whoever Linear names as its creator, and
// only the owner's prompts in it count. The machine checks the chain against the parent as
// read (docs/state-machine.md, The decision).
export function decide(payload: AgentSessionEventWebhookPayload, ownerUserId: string, random: () => number = Math.random, ownDelegation = false): Decision | null {
  const session = payload.agentSession;
  // An archived session is over. Posting to it would only bring it back.
  if (!session?.id || session.archivedAt) return null;

  const agentSessionId = session.id;
  const reply = (type: "thought" | "response", body: string): ActivityInput => ({ agentSessionId, content: { type, body } });
  const ack = (state: "acknowledging" | "heard" | "stopping"): ActivityInput => ({ ...reply("thought", phrase(state, random)), ephemeral: true });
  // Who started this session: whoever delegated the issue, or wrote the comment that
  // mentioned Tolkster. Sessions started by automations or other agents have no
  // creator, and are refused.
  const creatorId = session.creatorId ?? session.creator?.id;
  const issueId = session.issueId ?? session.issue?.id;

  // A mention's session carries the comment that mentioned Tolkster, or, for a reply in
  // another thread, that thread's root, which may have no author, and a `sourceCommentId`.
  // A delegation's has a comment too, the thread Linear opens for it, but Linear writes
  // that one itself, so it has no author and there is no source comment. The CLI tells
  // them apart the same way (`mentioning_comment` in `cli/src/sessions.rs`). A mention in
  // a new issue's description looks like a delegation's here: only the issue's creation,
  // creator and history, none of them in this payload, tell it apart, so it is acknowledged
  // and handed on as one.
  const mention = Boolean(session.comment?.userId || session.sourceCommentId);
  // Tolkster opened it itself, on a comment of its own, which `tolkster listen` does only for
  // the owner's reply there (`ports::comment_kind`); the CLI trusts the same shape
  // (`on_own_note` in `cli/src/sessions.rs`) and takes only the owner's replies as its
  // request. Linear gives a session an agent opens no creator.
  const ownNote = !creatorId && !session.sourceCommentId && Boolean(payload.appUserId) && session.comment?.userId === payload.appUserId;
  // A mention's session asks what its comment's author asked, so a delegation's only.
  const trust: SessionTrust | null = creatorId === ownerUserId ? "owner" : ownNote ? "reply" : ownDelegation && !mention ? "chain" : null;
  if (!trust) {
    // Only the "created" event gets an answer. A refused session would otherwise be
    // answered again on every reply.
    if (payload.action !== "created") return null;
    // Only a delegation is handed back. A mention says nothing about who delegated the
    // issue, and clearing it would let anyone stop the owner's work with one comment.
    return { kind: "not_owner", activity: reply("response", NOT_OWNER), ...(issueId && creatorId && payload.appUserId && !mention ? { undelegate: { issueId, creatorId, appUserId: payload.appUserId } } : {}) };
  }

  if (!issueId) {
    if (payload.action !== "created" && payload.action !== "prompted") return null;
    return { kind: "no_issue", activity: reply("response", NO_ISSUE) };
  }

  // As `comment_kind` (cli/src/ports.rs) finds a mention's comment.
  const commentId = trust !== "owner" ? null : session.sourceCommentId ?? (session.comment?.userId ? session.comment.id : null);
  const request = (at: string): SessionRequest => ({ trust, sessionKind: mention ? "mention" : "delegation", commentId: commentId ?? null, identifier: session.issue?.identifier ?? null, at });
  switch (payload.action) {
    case "created": {
      return { kind: "start", activity: ack(mention ? "heard" : "acknowledging"), request: request(session.createdAt) };
    }
    case "prompted": {
      // Anyone can reply in a thread the owner started. Only the owner's prompts are work.
      const prompt = payload.agentActivity;
      const authorId = prompt?.userId ?? prompt?.user?.id;
      if (!prompt || authorId !== ownerUserId) return { kind: "not_owner", activity: reply("response", NOT_OWNER) };
      const type = prompt.signal === "stop" ? "stop" : "deliver";
      return { kind: type, activity: ack(type === "stop" ? "stopping" : "heard"), request: request(prompt.createdAt) };
    }
    default:
      return null;
  }
}

export type Presence = "listening" | "away" | "unknown";

// The activity to post for a decision: its own, or AWAY when the decision carries a
// request and nothing is listening for it. `listening` is asked once, and answered within
// `deadlineMs` or not at all: a relay that fails or is slow leaves the decision's own
// acknowledgement, so it never holds the Worker past Linear's ten seconds. A refusal
// carries no request and asks nothing. Presence changes the wording only: the event is the
// same either way (index.ts, `act`).
export async function acknowledgement(decision: Decision, listening: () => Promise<boolean>, deadlineMs: number): Promise<{ activity: ActivityInput; presence: Presence | null }> {
  if (!decision.request) return { activity: decision.activity, presence: null };
  const presence = await within(deadlineMs, listening).then((heard) => (heard === null ? "unknown" : heard ? "listening" : "away"), () => "unknown" as const);
  if (presence !== "away") return { activity: decision.activity, presence };
  return { activity: { agentSessionId: decision.activity.agentSessionId, content: { type: "response", body: AWAY } }, presence };
}

// `call`'s answer, or null once `deadlineMs` have passed without one.
function within<T>(deadlineMs: number, call: () => Promise<T>): Promise<T | null> {
  let timer: ReturnType<typeof setTimeout> | null = null;
  const late = new Promise<null>((resolve) => {
    timer = setTimeout(() => resolve(null), deadlineMs);
  });
  return Promise.race([call(), late]).finally(() => {
    if (timer !== null) clearTimeout(timer);
  });
}
