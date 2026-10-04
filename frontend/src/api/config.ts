import { Platform } from "react-native";

// Web is served on styrta.energia.dev, which proxies /v1 to the API.
// A relative base keeps the browser on one origin.
export const API_BASE_URL =
  Platform.OS === "web" ? "" : "https://api.styrta.energia.dev";
