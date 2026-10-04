import { Platform } from "react-native";

import { API_BASE_URL } from "./config";
import { clearStoredSession, loadStoredSession, saveStoredSession } from "./session";
import {
  ApiError,
  type AcceptedTurn,
  type ChatStreamEvent,
  type HistoryMessage,
  type MyEvent,
  type NearbyEvent,
  type Refreshed,
  type Session,
  type TurnView,
} from "./types";

export async function signUp(email: string, password: string): Promise<Session> {
  const response = await fetch(`${API_BASE_URL}/v1/auth/register`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      email: email.trim(),
      password,
      display_name: displayNameFromEmail(email),
      locale: "en",
    }),
  });
  const session = await readJson(response, 200, parseSession);
  await saveStoredSession(session);
  return session;
}

export async function signIn(email: string, password: string): Promise<Session> {
  const response = await fetch(`${API_BASE_URL}/v1/auth/login`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      email: email.trim(),
      password,
    }),
  });
  const session = await readJson(response, 200, parseSession);
  await saveStoredSession(session);
  return session;
}

function displayNameFromEmail(email: string): string {
  const local = email.trim().split("@")[0]?.trim() ?? "";
  return local.length > 0 ? local.slice(0, 80) : "Tester";
}

export function getMyEvents(accessToken: string): Promise<MyEvent[]> {
  return authorizedJson(accessToken, "/v1/me/events", undefined, 200, (body, status) =>
    parseArray(body, status, parseMyEvent),
  );
}

export function getNearbyEvents(
  accessToken: string,
  query: { lat: number; lng: number; zoom: number; bbox?: string },
): Promise<NearbyEvent[]> {
  if (!Number.isFinite(query.lat) || !Number.isFinite(query.lng) || !Number.isFinite(query.zoom)) {
    return Promise.reject(new ApiError(400, "query"));
  }
  const params = new URLSearchParams();
  params.set("lat", String(query.lat));
  params.set("lng", String(query.lng));
  params.set("zoom", String(query.zoom));
  if (query.bbox !== undefined) {
    params.set("bbox", query.bbox);
  }
  return authorizedJson(
    accessToken,
    `/v1/events/nearby?${params.toString()}`,
    undefined,
    200,
    (body, status) => parseArray(body, status, parseNearbyEvent),
  );
}

export function listMessages(accessToken: string, limit?: number): Promise<HistoryMessage[]> {
  const pageSize = limit ?? 50;
  if (!Number.isInteger(pageSize) || pageSize <= 0) {
    return Promise.reject(new ApiError(400, "limit"));
  }
  const params = new URLSearchParams();
  params.set("limit", String(pageSize));
  return authorizedJson(
    accessToken,
    `/v1/chat/messages?${params.toString()}`,
    undefined,
    200,
    (body, status) => parseArray(body, status, parseHistoryMessage),
  );
}

export function postMessage(accessToken: string, text: string): Promise<AcceptedTurn> {
  return authorizedJson(
    accessToken,
    "/v1/chat/messages",
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ text }),
    },
    202,
    parseAcceptedTurn,
  );
}

export type SpeechClip = {
  uri: string;
  name: string;
  type: string;
};

export async function postAudioMessage(
  accessToken: string,
  clip: SpeechClip,
): Promise<AcceptedTurn> {
  const first = await sendAudio(accessToken, clip);
  if (first.status === 401) {
    await drain(first);
    const nextToken = await refreshAccessToken();
    const second = await sendAudio(nextToken, clip);
    return readJson(second, 202, parseAcceptedTurn);
  }
  return readJson(first, 202, parseAcceptedTurn);
}

export function spokenReplySource(
  accessToken: string,
  url: string,
): { uri: string; headers: Record<string, string> } {
  const path = url.startsWith("/") ? url : `/${url}`;
  const uri = url.startsWith("http") ? url : `${API_BASE_URL}${path}`;
  return {
    uri,
    headers: { Authorization: `Bearer ${accessToken}` },
  };
}

async function sendAudio(accessToken: string, clip: SpeechClip): Promise<Response> {
  return fetch(`${API_BASE_URL}/v1/chat/messages`, {
    method: "POST",
    headers: { Authorization: `Bearer ${accessToken}` },
    body: await audioBody(clip),
  });
}

async function audioBody(clip: SpeechClip): Promise<FormData> {
  const body = new FormData();
  if (Platform.OS === "web") {
    const response = await fetch(clip.uri);
    const blob = await response.blob();
    const type = blob.type || clip.type;
    const file = blob.type === type ? blob : new Blob([blob], { type });
    body.append("audio", file, clip.name);
    return body;
  }
  body.append("audio", {
    uri: clip.uri,
    name: clip.name,
    type: clip.type,
  } as unknown as Blob);
  return body;
}

export function getTurn(accessToken: string, turnId: string): Promise<TurnView> {
  return authorizedJson(
    accessToken,
    `/v1/chat/turns/${encodeURIComponent(turnId)}`,
    undefined,
    200,
    parseTurnView,
  );
}

export function openChatStream(
  accessToken: string,
  onEvent: (event: ChatStreamEvent) => void,
  onError: (error: Error) => void,
): { close: () => void } {
  let active: XMLHttpRequest | null = null;
  let closed = false;
  let refreshed = false;
  let reported = false;
  let consumed = 0;

  const report = (error: Error) => {
    if (closed || reported) {
      return;
    }
    reported = true;
    onError(error);
  };

  const consume = (request: XMLHttpRequest) => {
    if (request.status !== 200) {
      return;
    }
    const text = request.responseText;
    if (text.length < consumed) {
      consumed = 0;
    }
    const fresh = text.slice(consumed);
    const parts = fresh.split("\n\n");
    const remainder = parts.pop() ?? "";
    consumed = text.length - remainder.length;
    for (const part of parts) {
      const event = decodeSseBlock(part);
      if (event) {
        onEvent(event);
      }
    }
  };

  const open = (token: string) => {
    consumed = 0;
    const request = new XMLHttpRequest();
    active = request;
    request.open("GET", `${API_BASE_URL}/v1/stream`);
    request.setRequestHeader("Authorization", `Bearer ${token}`);
    request.setRequestHeader("Accept", "text/event-stream");

    const isCurrent = () => request === active && !closed;

    request.onprogress = () => {
      if (!isCurrent()) {
        return;
      }
      consume(request);
    };

    request.onreadystatechange = () => {
      if (!isCurrent()) {
        return;
      }
      const headersOrDone =
        request.readyState === XMLHttpRequest.HEADERS_RECEIVED ||
        request.readyState === XMLHttpRequest.DONE;
      if (request.status === 401 && headersOrDone) {
        const dying = request;
        active = null;
        dying.abort();
        void recover();
        return;
      }
      if (
        request.status === 200 &&
        (request.readyState === XMLHttpRequest.LOADING ||
          request.readyState === XMLHttpRequest.DONE)
      ) {
        consume(request);
      }
      if (request.readyState === XMLHttpRequest.DONE && request.status === 0) {
        report(new ApiError(0, "network"));
      } else if (
        request.readyState === XMLHttpRequest.DONE &&
        request.status !== 200 &&
        request.status !== 401
      ) {
        report(errorFromXhr(request));
      }
    };

    request.onerror = () => {
      if (!isCurrent()) {
        return;
      }
      report(new ApiError(0, "network"));
    };

    request.send();
  };

  async function recover(): Promise<void> {
    if (closed) {
      return;
    }
    if (refreshed) {
      await clearStoredSession();
      report(new ApiError(401, "unauthorized"));
      return;
    }
    refreshed = true;
    try {
      const token = await refreshAccessToken();
      if (closed) {
        return;
      }
      open(token);
    } catch (error) {
      report(error instanceof Error ? error : new ApiError(401, "unauthorized"));
    }
  }

  open(accessToken);

  return {
    close() {
      if (closed) {
        return;
      }
      closed = true;
      const request = active;
      active = null;
      request?.abort();
    },
  };
}

let refreshInFlight: Promise<string> | null = null;

function refreshAccessToken(): Promise<string> {
  if (!refreshInFlight) {
    refreshInFlight = refreshAccessTokenOnce().finally(() => {
      refreshInFlight = null;
    });
  }
  return refreshInFlight;
}

async function refreshAccessTokenOnce(): Promise<string> {
  const stored = await loadStoredSession();
  if (!stored?.refresh_token) {
    await clearStoredSession();
    throw new ApiError(401, "unauthorized");
  }
  try {
    const refreshed = await postRefresh(stored.refresh_token);
    const session = mergeSession(stored, refreshed);
    await saveStoredSession(session);
    return session.access_token;
  } catch (error) {
    await clearStoredSession();
    if (error instanceof ApiError) {
      throw error;
    }
    throw new ApiError(401, "unauthorized");
  }
}

async function postRefresh(refreshToken: string): Promise<Refreshed> {
  const response = await fetch(`${API_BASE_URL}/v1/auth/refresh`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ refresh_token: refreshToken }),
  });
  return readJson(response, 200, parseRefreshed);
}

function mergeSession(stored: Session, refreshed: Refreshed): Session {
  return {
    id: stored.id,
    email: stored.email,
    display_name: stored.display_name,
    access_token: refreshed.access_token,
    refresh_token: refreshed.refresh_token,
    access_expires_at: refreshed.access_expires_at,
    refresh_expires_at: refreshed.refresh_expires_at,
  };
}

async function authorizedJson<T>(
  accessToken: string,
  path: string,
  init: RequestInit | undefined,
  successStatus: number,
  parse: (body: unknown, status: number) => T,
): Promise<T> {
  const response = await authorizedFetch(accessToken, path, init, successStatus);
  return readJson(response, successStatus, parse);
}

async function authorizedFetch(
  accessToken: string,
  path: string,
  init: RequestInit | undefined,
  successStatus: number,
): Promise<Response> {
  const url = `${API_BASE_URL}${path}`;
  const first = await fetch(url, withAccessToken(accessToken, init));
  if (first.status === 401) {
    await drain(first);
    const nextToken = await refreshAccessToken();
    const second = await fetch(url, withAccessToken(nextToken, init));
    if (second.status !== successStatus) {
      throw await apiErrorFrom(second);
    }
    return second;
  }
  if (first.status !== successStatus) {
    throw await apiErrorFrom(first);
  }
  return first;
}

function withAccessToken(accessToken: string, init: RequestInit | undefined): RequestInit {
  const headers = new Headers(init?.headers);
  headers.set("Authorization", `Bearer ${accessToken}`);
  return { ...init, headers };
}

async function readJson<T>(
  response: Response,
  successStatus: number,
  parse: (body: unknown, status: number) => T,
): Promise<T> {
  if (response.status !== successStatus) {
    throw await apiErrorFrom(response);
  }
  let body: unknown;
  try {
    body = (await response.json()) as unknown;
  } catch {
    throw new ApiError(response.status, "response");
  }
  return parse(body, response.status);
}

async function apiErrorFrom(response: Response): Promise<ApiError> {
  let message = "request failed";
  try {
    const body = (await response.json()) as unknown;
    const record = asRecord(body);
    if (record && typeof record.error === "string" && record.error.length > 0) {
      message = record.error;
    }
  } catch {
    // Non-JSON error bodies keep the short fallback.
  }
  return new ApiError(response.status, message);
}

async function drain(response: Response): Promise<void> {
  try {
    await response.text();
  } catch {
    // Ignore a body that cannot be read.
  }
}

function parseSession(body: unknown, status: number): Session {
  const record = expectRecord(body, status);
  return {
    id: expectString(record, "id", status),
    email: expectString(record, "email", status),
    display_name: expectString(record, "display_name", status),
    access_token: expectString(record, "access_token", status),
    refresh_token: expectString(record, "refresh_token", status),
    access_expires_at: expectString(record, "access_expires_at", status),
    refresh_expires_at: expectString(record, "refresh_expires_at", status),
  };
}

function parseRefreshed(body: unknown, status: number): Refreshed {
  const record = expectRecord(body, status);
  return {
    access_token: expectString(record, "access_token", status),
    refresh_token: expectString(record, "refresh_token", status),
    access_expires_at: expectString(record, "access_expires_at", status),
    refresh_expires_at: expectString(record, "refresh_expires_at", status),
  };
}

function parseNearbyEvent(body: unknown, status: number): NearbyEvent {
  const record = expectRecord(body, status);
  return {
    id: expectString(record, "id", status),
    title: expectString(record, "title", status),
    emoji: expectString(record, "emoji", status),
    signed_count: expectNumber(record, "signed_count", status),
    capacity: expectNumberOrNull(record, "capacity", status),
    place_name: expectString(record, "place_name", status),
    latitude: expectNumber(record, "latitude", status),
    longitude: expectNumber(record, "longitude", status),
    starts_at: expectString(record, "starts_at", status),
    host_name: expectString(record, "host_name", status),
  };
}

function parseMyEvent(body: unknown, status: number): MyEvent {
  const record = expectRecord(body, status);
  const statusValue = expectString(record, "status", status);
  if (statusValue !== "scheduled" && statusValue !== "cancelled") {
    throw new ApiError(status, "response");
  }
  return {
    id: expectString(record, "id", status),
    host_id: expectString(record, "host_id", status),
    host_name: expectString(record, "host_name", status),
    title: expectString(record, "title", status),
    emoji: expectString(record, "emoji", status),
    description: expectStringOrNull(record, "description", status),
    starts_at: expectString(record, "starts_at", status),
    capacity: expectNumberOrNull(record, "capacity", status),
    activity_tags: expectStringArray(record, "activity_tags", status),
    promoted: expectBoolean(record, "promoted", status),
    status: statusValue,
    women_only: expectBoolean(record, "women_only", status),
    place_name: expectString(record, "place_name", status),
    place_kind: expectString(record, "place_kind", status),
    latitude: expectNumber(record, "latitude", status),
    longitude: expectNumber(record, "longitude", status),
    signed_count: expectNumber(record, "signed_count", status),
  };
}

function parseHistoryMessage(body: unknown, status: number): HistoryMessage {
  const record = expectRecord(body, status);
  const role = expectString(record, "role", status);
  if (role !== "user" && role !== "assistant") {
    throw new ApiError(status, "response");
  }
  return {
    id: expectString(record, "id", status),
    role,
    body: expectString(record, "body", status),
    created_at: expectString(record, "created_at", status),
  };
}

function parseAcceptedTurn(body: unknown, status: number): AcceptedTurn {
  const record = expectRecord(body, status);
  return { turn_id: expectString(record, "turn_id", status) };
}

function parseTurnView(body: unknown, status: number): TurnView {
  const record = expectRecord(body, status);
  const turnStatus = expectString(record, "status", status);
  if (turnStatus !== "running" && turnStatus !== "done" && turnStatus !== "failed") {
    throw new ApiError(status, "response");
  }
  return {
    status: turnStatus,
    user_text: expectStringOrNull(record, "user_text", status),
    reply_text: expectStringOrNull(record, "reply_text", status),
    audio_ready: expectBoolean(record, "audio_ready", status),
  };
}

function parseArray<T>(
  body: unknown,
  status: number,
  parseItem: (value: unknown, status: number) => T,
): T[] {
  if (!Array.isArray(body)) {
    throw new ApiError(status, "response");
  }
  return body.map((item) => parseItem(item, status));
}

function expectRecord(value: unknown, status: number): Record<string, unknown> {
  const record = asRecord(value);
  if (!record) {
    throw new ApiError(status, "response");
  }
  return record;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (typeof value === "object" && value !== null && !Array.isArray(value)) {
    return value as Record<string, unknown>;
  }
  return null;
}

function expectString(record: Record<string, unknown>, key: string, status: number): string {
  const value = record[key];
  if (typeof value !== "string") {
    throw new ApiError(status, "response");
  }
  return value;
}

function expectStringOrNull(
  record: Record<string, unknown>,
  key: string,
  status: number,
): string | null {
  const value = record[key];
  if (value === null) {
    return null;
  }
  if (typeof value !== "string") {
    throw new ApiError(status, "response");
  }
  return value;
}

function expectNumber(record: Record<string, unknown>, key: string, status: number): number {
  const value = record[key];
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new ApiError(status, "response");
  }
  return value;
}

function expectNumberOrNull(
  record: Record<string, unknown>,
  key: string,
  status: number,
): number | null {
  const value = record[key];
  if (value === null) {
    return null;
  }
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new ApiError(status, "response");
  }
  return value;
}

function expectBoolean(record: Record<string, unknown>, key: string, status: number): boolean {
  const value = record[key];
  if (typeof value !== "boolean") {
    throw new ApiError(status, "response");
  }
  return value;
}

function expectStringArray(record: Record<string, unknown>, key: string, status: number): string[] {
  const value = record[key];
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string")) {
    throw new ApiError(status, "response");
  }
  return value as string[];
}

function decodeSseBlock(block: string): ChatStreamEvent | null {
  let eventName = "";
  const dataLines: string[] = [];
  let sawField = false;
  for (const rawLine of block.split("\n")) {
    const line = rawLine.endsWith("\r") ? rawLine.slice(0, -1) : rawLine;
    if (line.length === 0 || line.startsWith(":")) {
      continue;
    }
    sawField = true;
    const colon = line.indexOf(":");
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? "" : line.slice(colon + 1);
    if (value.startsWith(" ")) {
      value = value.slice(1);
    }
    if (field === "event") {
      eventName = value;
    } else if (field === "data") {
      dataLines.push(value);
    }
  }
  if (!sawField || eventName.length === 0 || dataLines.length === 0) {
    return null;
  }
  let payload: unknown;
  try {
    payload = JSON.parse(dataLines.join("\n")) as unknown;
  } catch {
    return null;
  }
  return decodeStreamEvent(eventName, payload);
}

function decodeStreamEvent(eventName: string, payload: unknown): ChatStreamEvent | null {
  const record = asRecord(payload);
  if (!record) {
    return null;
  }
  const turnId = record.turn_id;
  if (typeof turnId !== "string") {
    return null;
  }
  switch (eventName) {
    case "turn.started":
    case "turn.done":
      return { type: eventName, turn_id: turnId };
    case "transcript.ready":
    case "reply.delta":
    case "reply.done": {
      if (typeof record.text !== "string") {
        return null;
      }
      return { type: eventName, turn_id: turnId, text: record.text };
    }
    case "turn.failed": {
      if (typeof record.error !== "string") {
        return null;
      }
      return { type: "turn.failed", turn_id: turnId, error: record.error };
    }
    case "audio.ready": {
      if (typeof record.url !== "string") {
        return null;
      }
      return { type: "audio.ready", turn_id: turnId, url: record.url };
    }
    case "tool.started": {
      if (typeof record.tool !== "string" || typeof record.label !== "string") {
        return null;
      }
      return { type: "tool.started", turn_id: turnId, tool: record.tool, label: record.label };
    }
    case "tool.finished": {
      const eventId = record.event_id;
      const hitCount = record.hit_count;
      if (typeof record.tool !== "string" || typeof record.ok !== "boolean") {
        return null;
      }
      if (eventId !== null && typeof eventId !== "string") {
        return null;
      }
      if (hitCount !== null && (typeof hitCount !== "number" || !Number.isFinite(hitCount))) {
        return null;
      }
      return {
        type: "tool.finished",
        turn_id: turnId,
        tool: record.tool,
        ok: record.ok,
        event_id: eventId,
        hit_count: hitCount,
      };
    }
    default:
      return null;
  }
}

function errorFromXhr(request: XMLHttpRequest): ApiError {
  let message = "request failed";
  try {
    const body = JSON.parse(request.responseText) as unknown;
    const record = asRecord(body);
    if (record && typeof record.error === "string" && record.error.length > 0) {
      message = record.error;
    }
  } catch {
    // Non-JSON stream errors keep the short fallback.
  }
  return new ApiError(request.status, message);
}
