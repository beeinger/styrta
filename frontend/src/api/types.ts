export type Session = {
  id: string;
  email: string;
  display_name: string;
  access_token: string;
  refresh_token: string;
  access_expires_at: string;
  refresh_expires_at: string;
};

export type Refreshed = {
  access_token: string;
  refresh_token: string;
  access_expires_at: string;
  refresh_expires_at: string;
};

export type NearbyEvent = {
  id: string;
  title: string;
  emoji: string;
  signed_count: number;
  capacity: number | null;
  place_name: string;
  latitude: number;
  longitude: number;
  starts_at: string;
  host_name: string;
};

export type MyEvent = {
  id: string;
  host_id: string;
  host_name: string;
  title: string;
  emoji: string;
  description: string | null;
  starts_at: string;
  capacity: number | null;
  activity_tags: string[];
  promoted: boolean;
  status: "scheduled" | "cancelled";
  women_only: boolean;
  place_name: string;
  place_kind: string;
  latitude: number;
  longitude: number;
  signed_count: number;
};

export type HistoryMessage = {
  id: string;
  role: "user" | "assistant";
  body: string;
  created_at: string;
};

export type AcceptedTurn = {
  turn_id: string;
};

export type TurnView = {
  status: "running" | "done" | "failed";
  user_text: string | null;
  reply_text: string | null;
  audio_ready: boolean;
};

/**
 * `reply.delta` `text` is a chunk to append.
 * `reply.done` `text` is the full reply and replaces assembled chunks.
 */
export type ChatStreamEvent =
  | { type: "turn.started"; turn_id: string }
  | { type: "transcript.ready"; turn_id: string; text: string }
  | { type: "tool.started"; turn_id: string; tool: string; label: string }
  | {
      type: "tool.finished";
      turn_id: string;
      tool: string;
      ok: boolean;
      event_id: string | null;
      hit_count: number | null;
    }
  | { type: "reply.delta"; turn_id: string; text: string }
  | { type: "reply.done"; turn_id: string; text: string }
  | { type: "audio.ready"; turn_id: string; url: string }
  | { type: "turn.done"; turn_id: string }
  | { type: "turn.failed"; turn_id: string; error: string };

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}
