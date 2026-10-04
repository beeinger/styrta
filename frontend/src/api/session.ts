import { deleteItem, getItem, setItem } from "./kv";
import type { Session } from "./types";

const ACCESS_TOKEN_KEY = "styrta.access_token";
const REFRESH_TOKEN_KEY = "styrta.refresh_token";
const SESSION_META_KEY = "styrta.session_meta";

type SessionMeta = {
  id: string;
  email: string;
  display_name: string;
  access_expires_at: string;
  refresh_expires_at: string;
};

export async function loadStoredSession(): Promise<Session | null> {
  const [accessToken, refreshToken, metaJson] = await Promise.all([
    getItem(ACCESS_TOKEN_KEY),
    getItem(REFRESH_TOKEN_KEY),
    getItem(SESSION_META_KEY),
  ]);
  if (!accessToken || !refreshToken || !metaJson) {
    return null;
  }
  const meta = parseMeta(metaJson);
  if (!meta) {
    return null;
  }
  return {
    id: meta.id,
    email: meta.email,
    display_name: meta.display_name,
    access_token: accessToken,
    refresh_token: refreshToken,
    access_expires_at: meta.access_expires_at,
    refresh_expires_at: meta.refresh_expires_at,
  };
}

export async function saveStoredSession(session: Session): Promise<void> {
  const meta: SessionMeta = {
    id: session.id,
    email: session.email,
    display_name: session.display_name,
    access_expires_at: session.access_expires_at,
    refresh_expires_at: session.refresh_expires_at,
  };
  await Promise.all([
    setItem(ACCESS_TOKEN_KEY, session.access_token),
    setItem(REFRESH_TOKEN_KEY, session.refresh_token),
    setItem(SESSION_META_KEY, JSON.stringify(meta)),
  ]);
}

export async function clearStoredSession(): Promise<void> {
  await Promise.all([
    deleteItem(ACCESS_TOKEN_KEY),
    deleteItem(REFRESH_TOKEN_KEY),
    deleteItem(SESSION_META_KEY),
  ]);
}

function parseMeta(raw: string): SessionMeta | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw) as unknown;
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return null;
  }
  const record = parsed as Record<string, unknown>;
  const id = record.id;
  const email = record.email;
  const displayName = record.display_name;
  const accessExpiresAt = record.access_expires_at;
  const refreshExpiresAt = record.refresh_expires_at;
  if (
    typeof id !== "string" ||
    typeof email !== "string" ||
    typeof displayName !== "string" ||
    typeof accessExpiresAt !== "string" ||
    typeof refreshExpiresAt !== "string" ||
    id.length === 0 ||
    displayName.length === 0
  ) {
    return null;
  }
  return {
    id,
    email,
    display_name: displayName,
    access_expires_at: accessExpiresAt,
    refresh_expires_at: refreshExpiresAt,
  };
}
