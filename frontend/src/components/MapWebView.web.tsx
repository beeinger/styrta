import { createElement, forwardRef, useEffect, useImperativeHandle, useRef } from "react";
import {
  View,
  type LayoutChangeEvent,
  type StyleProp,
  type ViewStyle,
} from "react-native";

export type WebViewMessageEvent = {
  nativeEvent: { data: string };
};

export type WebViewHandle = {
  injectJavaScript: (script: string) => void;
};

type Props = {
  source?: { html?: string; baseUrl?: string };
  style?: StyleProp<ViewStyle>;
  onMessage?: (event: WebViewMessageEvent) => void;
  onError?: (event: unknown) => void;
  onLayout?: (event: LayoutChangeEvent) => void;
  accessibilityLabel?: string;
  accessibilityHint?: string;
  originWhitelist?: string[];
  javaScriptEnabled?: boolean;
  domStorageEnabled?: boolean;
  scrollEnabled?: boolean;
  bounces?: boolean;
  overScrollMode?: string;
  scalesPageToFit?: boolean;
  setBuiltInZoomControls?: boolean;
  setDisplayZoomControls?: boolean;
  showsHorizontalScrollIndicator?: boolean;
  showsVerticalScrollIndicator?: boolean;
  allowsLinkPreview?: boolean;
  textZoom?: number;
  androidLayerType?: string;
  onShouldStartLoadWithRequest?: (request: { navigationType?: string }) => boolean;
};

const BRIDGE =
  '<script>window.ReactNativeWebView={postMessage:function(data){parent.postMessage(String(data),"*");}};</script>';

export const WebView = forwardRef<WebViewHandle, Props>(function WebView(props, ref) {
  const frameRef = useRef<HTMLIFrameElement | null>(null);
  const queued = useRef<string[]>([]);
  const onMessage = props.onMessage;
  const html = props.source?.html;

  const run = (script: string) => {
    const doc = frameRef.current?.contentDocument;
    if (!doc?.body) {
      queued.current.push(script);
      return;
    }
    const element = doc.createElement("script");
    element.text = script;
    doc.body.appendChild(element);
  };

  useImperativeHandle(ref, () => ({ injectJavaScript: run }), []);

  useEffect(() => {
    const handler = (event: MessageEvent) => {
      if (event.source !== frameRef.current?.contentWindow) {
        return;
      }
      if (typeof event.data !== "string") {
        return;
      }
      onMessage?.({ nativeEvent: { data: event.data } });
    };
    window.addEventListener("message", handler);
    return () => window.removeEventListener("message", handler);
  }, [onMessage]);

  return (
    <View
      style={props.style}
      onLayout={props.onLayout}
      accessibilityLabel={props.accessibilityLabel}
      accessibilityHint={props.accessibilityHint}
    >
      {html
        ? createElement("iframe", {
            ref: (node: HTMLIFrameElement | null) => {
              frameRef.current = node;
            },
            title: props.accessibilityLabel ?? "Map",
            srcDoc: BRIDGE + html,
            onLoad: () => {
              const pending = queued.current;
              queued.current = [];
              for (const script of pending) {
                run(script);
              }
            },
            style: {
              position: "absolute",
              top: 0,
              left: 0,
              width: "100%",
              height: "100%",
              border: "none",
            },
          })
        : null}
    </View>
  );
});
