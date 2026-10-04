import * as SecureStore from "expo-secure-store";

export function getItem(key: string): Promise<string | null> {
  return SecureStore.getItemAsync(key);
}

export function setItem(key: string, value: string): Promise<void> {
  return SecureStore.setItemAsync(key, value);
}

export function deleteItem(key: string): Promise<void> {
  return SecureStore.deleteItemAsync(key);
}
