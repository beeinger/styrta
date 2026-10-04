import * as SecureStore from "expo-secure-store";

import type { Session } from "./types";

const ACCESS_TOKEN_KEY = "styrta.access_token";
const REFRESH_TOKEN_KEY = "styrta.refresh_token";
const SESSION_META_KEY = "styrta.session_meta";

type SessionMeta = {
  id: string;
  display_name: string;
  access_expires_at: string;
  refresh_expires_at: string;
};

export async function loadStoredSession(): Promise<Session | null> {
  const [accessToken, refreshToken, metaJson] = await Promise.all([
    SecureStore.getItemAsync(ACCESS_TOKEN_KEY),
    SecureStore.getItemAsync(REFRESH_TOKEN_KEY),
    SecureStore.getItemAsync(SESSION_META_KEY),
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
    display_name: session.display_name,
    access_expires_at: session.access_expires_at,
    refresh_expires_at: session.refresh_expires_at,
  };
  await Promise.all([
    SecureStore.setItemAsync(ACCESS_TOKEN_KEY, session.access_token),
    SecureStore.setItemAsync(REFRESH_TOKEN_KEY, session.refresh_token),
    SecureStore.setItemAsync(SESSION_META_KEY, JSON.stringify(meta)),
  ]);
}

export async function clearStoredSession(): Promise<void> {
  await Promise.all([
    SecureStore.deleteItemAsync(ACCESS_TOKEN_KEY),
    SecureStore.deleteItemAsync(REFRESH_TOKEN_KEY),
    SecureStore.deleteItemAsync(SESSION_META_KEY),
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
  const displayName = record.display_name;
  const accessExpiresAt = record.access_expires_at;
  const refreshExpiresAt = record.refresh_expires_at;
  if (
    typeof id !== "string" ||
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
    display_name: displayName,
    access_expires_at: accessExpiresAt,
    refresh_expires_at: refreshExpiresAt,
  };
}
