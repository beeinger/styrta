export { WebView, type WebViewMessageEvent } from "react-native-webview";

export type WebViewHandle = {
  injectJavaScript: (script: string) => void;
};
