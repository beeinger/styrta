export function getItem(key: string): Promise<string | null> {
  try {
    return Promise.resolve(window.localStorage.getItem(key));
  } catch {
    return Promise.resolve(null);
  }
}

export function setItem(key: string, value: string): Promise<void> {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Private mode and blocked storage leave the session in memory only.
  }
  return Promise.resolve();
}

export function deleteItem(key: string): Promise<void> {
  try {
    window.localStorage.removeItem(key);
  } catch {
    // Same as setItem: a missing store is not a failed sign-out.
  }
  return Promise.resolve();
}
