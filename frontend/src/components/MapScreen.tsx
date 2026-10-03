import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AccessibilityInfo,
  ActivityIndicator,
  AppState,
  I18nManager,
  Linking,
  Platform,
  Pressable,
  StyleSheet,
  Text,
  View,
} from "react-native";
import * as Location from "expo-location";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { WebView, type WebViewMessageEvent } from "react-native-webview";

import { AiChatSheet } from "./AiChatSheet";
import { createMapHtml, type MapMarker, type MapPadding } from "./mapDocument";
import { MapZoomControls } from "./MapZoomControls";

const FALLBACK_CAMERA = {
  latitude: 37.78825,
  longitude: -122.4324,
  zoom: 13,
};

const USER_ZOOM = 14;

type Camera = {
  latitude: number;
  longitude: number;
  zoom: number;
};

type Coordinates = {
  latitude: number;
  longitude: number;
};

const MIN_ZOOM = 3;
const MAX_ZOOM = 18;

// Add a pin by appending { id, latitude, longitude, title }. Tapping it shows the title.
const markers: MapMarker[] = [];

type MapMessage =
  | { type: "ready"; zoom: number }
  | { type: "zoom"; zoom: number; source?: "control" | "gesture" }
  | { type: "zoom-blocked"; direction: "in" | "out" }
  | { type: "marker-press"; id: string }
  | { type: "map-press" }
  | { type: "error"; message?: string };

export function MapScreen() {
  const insets = useSafeAreaInsets();
  const webViewRef = useRef<WebView>(null);
  const announcedReady = useRef(false);
  const announcedLocation = useRef(false);
  const reduceMotionRef = useRef(false);
  const [startCamera, setStartCamera] = useState<Camera | null>(null);
  const [userLocation, setUserLocation] = useState<Coordinates | null>(null);
  const [zoom, setZoom] = useState(FALLBACK_CAMERA.zoom);
  const [sheetHeight, setSheetHeight] = useState(0);
  const [mapReady, setMapReady] = useState(false);
  const [mapFailed, setMapFailed] = useState(false);
  const [reduceMotion, setReduceMotion] = useState(false);
  const [selectedMarker, setSelectedMarker] = useState<MapMarker | null>(null);
  const [locationPrompt, setLocationPrompt] = useState<
    "hidden" | "ask" | "settings"
  >("hidden");
  const locating = useRef(false);
  const locationPromptRef = useRef(locationPrompt);
  locationPromptRef.current = locationPrompt;
  const obstruction = sheetHeight || 168;

  const html = useMemo(() => {
    if (!startCamera) {
      return null;
    }
    return createMapHtml({
      latitude: startCamera.latitude,
      longitude: startCamera.longitude,
      zoom: startCamera.zoom,
      minZoom: MIN_ZOOM,
      maxZoom: MAX_ZOOM,
    });
  }, [startCamera]);

  const centerOnUser = useCallback(async () => {
    if (locating.current) {
      return;
    }
    locating.current = true;
    try {
      const existing = await Location.getForegroundPermissionsAsync();
      const permission =
        existing.status === "granted"
          ? existing
          : await Location.requestForegroundPermissionsAsync();
      if (permission.status !== "granted") {
        setLocationPrompt(permission.canAskAgain ? "ask" : "settings");
        setStartCamera((current) => current ?? FALLBACK_CAMERA);
        AccessibilityInfo.announceForAccessibility(
          "Location access was denied. Showing a default area.",
        );
        return;
      }

      setLocationPrompt("hidden");

      if (Platform.OS === "android") {
        try {
          await Location.enableNetworkProviderAsync();
        } catch {
          // The position request below fails if location services stay off.
        }
      }

      const lastKnown = await Location.getLastKnownPositionAsync();
      if (lastKnown) {
        publishLocation(lastKnown.coords);
      }

      const current = await withTimeout(
        Location.getCurrentPositionAsync({
          accuracy: Location.Accuracy.Balanced,
        }),
        10000,
      );
      publishLocation(current.coords);
    } catch {
      setLocationPrompt((current) => (current === "hidden" ? "ask" : current));
      setStartCamera((current) => current ?? FALLBACK_CAMERA);
    } finally {
      locating.current = false;
    }
  }, []);

  function publishLocation(coords: Location.LocationObjectCoords) {
    const next = {
      latitude: coords.latitude,
      longitude: coords.longitude,
    };
    setUserLocation(next);
    setStartCamera((current) => current ?? { ...next, zoom: USER_ZOOM });
    setZoom((currentZoom) => Math.max(currentZoom, USER_ZOOM));
    if (!announcedLocation.current) {
      announcedLocation.current = true;
      AccessibilityInfo.announceForAccessibility("Showing your location");
    }
  }

  useEffect(() => {
    let mounted = true;

    void centerOnUser();

    AccessibilityInfo.isReduceMotionEnabled().then((enabled) => {
      if (mounted) {
        reduceMotionRef.current = enabled;
        setReduceMotion(enabled);
      }
    });

    const subscription = AccessibilityInfo.addEventListener(
      "reduceMotionChanged",
      (enabled) => {
        reduceMotionRef.current = enabled;
        setReduceMotion(enabled);
      },
    );

    return () => {
      mounted = false;
      subscription.remove();
    };
  }, [centerOnUser]);

  useEffect(() => {
    const timer = setTimeout(() => {
      if (announcedLocation.current) {
        return;
      }
      setStartCamera((current) => current ?? FALLBACK_CAMERA);
      setLocationPrompt((current) => (current === "hidden" ? "ask" : current));
    }, 8000);
    return () => clearTimeout(timer);
  }, []);

  useEffect(() => {
    const subscription = AppState.addEventListener("change", (state) => {
      if (state === "active" && locationPromptRef.current !== "hidden") {
        void centerOnUser();
      }
    });
    return () => subscription.remove();
  }, [centerOnUser]);

  const endInset = I18nManager.isRTL ? insets.left : insets.right;
  const startInset = I18nManager.isRTL ? insets.right : insets.left;
  const controlClearance = Math.max(16, endInset) + 64;

  const mapPadding = useMemo<MapPadding>(
    () => ({
      top: insets.top,
      bottom: obstruction,
      left: I18nManager.isRTL ? controlClearance : Math.max(16, startInset),
      right: I18nManager.isRTL ? Math.max(16, startInset) : controlClearance,
    }),
    [controlClearance, insets.top, obstruction, startInset],
  );

  useEffect(() => {
    if (!mapReady) {
      return;
    }
    run(`window.__styrtaMap.setPadding(${JSON.stringify(mapPadding)})`);
  }, [mapPadding, mapReady]);

  useEffect(() => {
    if (!mapReady) {
      return;
    }
    run(`window.__styrtaMap.setMarkers(${JSON.stringify(markers)})`);
  }, [mapReady]);

  useEffect(() => {
    if (!mapReady) {
      return;
    }
    run(
      `window.__styrtaMap.setSelected(${JSON.stringify(selectedMarker?.id ?? null)})`,
    );
  }, [mapReady, selectedMarker]);

  useEffect(() => {
    if (!mapReady || !userLocation) {
      return;
    }
    run(
      `window.__styrtaMap.setUserLocation(${userLocation.latitude}, ${userLocation.longitude}, ${reduceMotionRef.current ? "false" : "true"})`,
    );
  }, [mapReady, userLocation]);

  useEffect(() => {
    if (!startCamera || mapReady || mapFailed) {
      return;
    }

    const timer = setTimeout(() => setMapFailed(true), 15000);
    return () => clearTimeout(timer);
  }, [mapFailed, mapReady, startCamera]);

  const zoomBy = (direction: "in" | "out") => {
    if (!mapReady) {
      return;
    }
    const delta = direction === "in" ? 1 : -1;
    run(
      `window.__styrtaMap.zoomBy(${delta}, ${reduceMotion ? "false" : "true"})`,
    );
  };

  const onMessage = (event: WebViewMessageEvent) => {
    const message = parseMessage(event.nativeEvent.data);
    if (!message) {
      return;
    }

    if (message.type === "ready") {
      setZoom(message.zoom);
      setMapReady(true);
      setMapFailed(false);
      if (!announcedReady.current) {
        announcedReady.current = true;
        AccessibilityInfo.announceForAccessibility("Map loaded");
      }
      return;
    }

    if (message.type === "zoom") {
      setZoom(message.zoom);
      if (message.source === "control") {
        AccessibilityInfo.announceForAccessibility(
          message.zoom > zoom ? "Zoomed in" : "Zoomed out",
        );
      }
      return;
    }

    if (message.type === "zoom-blocked") {
      AccessibilityInfo.announceForAccessibility(
        message.direction === "in"
          ? "Already zoomed in as far as possible"
          : "Already zoomed out as far as possible",
      );
      return;
    }

    if (message.type === "marker-press") {
      const marker = markers.find((item) => item.id === message.id);
      if (!marker) {
        return;
      }
      setSelectedMarker(marker);
      AccessibilityInfo.announceForAccessibility(marker.title);
      return;
    }

    if (message.type === "map-press") {
      setSelectedMarker(null);
      return;
    }

    setMapFailed(true);
  };

  return (
    <View style={styles.screen}>
      {html ? (
        <WebView
          ref={webViewRef}
          source={{ html, baseUrl: "https://localhost" }}
          style={styles.map}
          originWhitelist={["*"]}
          javaScriptEnabled
          domStorageEnabled
          scrollEnabled={false}
          bounces={false}
          overScrollMode="never"
          scalesPageToFit={false}
          setBuiltInZoomControls={false}
          setDisplayZoomControls={false}
          showsHorizontalScrollIndicator={false}
          showsVerticalScrollIndicator={false}
          allowsLinkPreview={false}
          textZoom={100}
          androidLayerType="hardware"
          accessibilityLabel="Map"
          accessibilityHint="Pan with one finger to move the map. Pinch with two fingers to zoom, or use the zoom buttons."
          onMessage={onMessage}
          onError={() => setMapFailed(true)}
          onShouldStartLoadWithRequest={(request) =>
            request.navigationType !== "click"
          }
        />
      ) : null}
      {mapReady ? null : (
        <View style={styles.status} pointerEvents="none">
          <ActivityIndicator color="#1C1C1E" size="large" />
        </View>
      )}
      {mapFailed ? (
        <View style={styles.status} pointerEvents="none">
          <Text style={styles.statusText}>
            The map could not load. Check your internet connection and reopen
            the app.
          </Text>
        </View>
      ) : null}
      {locationPrompt === "hidden" ? null : (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={
            locationPrompt === "settings"
              ? "Enable location in Settings"
              : "Use my location"
          }
          onPress={() => {
            if (locationPrompt === "settings") {
              void Linking.openSettings();
              return;
            }
            void centerOnUser();
          }}
          style={[
            styles.locationButton,
            {
              top: insets.top + 12,
              start: Math.max(16, startInset),
              end: controlClearance + 12,
            },
          ]}
        >
          <Text style={styles.locationButtonText}>
            {locationPrompt === "settings"
              ? "Enable location in Settings"
              : "Use my location"}
          </Text>
        </Pressable>
      )}
      {selectedMarker ? (
        <View
          accessible
          accessibilityRole="text"
          accessibilityLabel={selectedMarker.title}
          style={[
            styles.callout,
            {
              bottom: obstruction + 12,
              start: Math.max(16, startInset),
              end: controlClearance + 12,
            },
          ]}
        >
          <Text style={styles.calloutText}>{selectedMarker.title}</Text>
        </View>
      ) : null}
      <MapZoomControls
        bottom={obstruction + 12}
        canZoomIn={mapReady && zoom < MAX_ZOOM - 0.01}
        canZoomOut={mapReady && zoom > MIN_ZOOM + 0.01}
        onZoomIn={() => zoomBy("in")}
        onZoomOut={() => zoomBy("out")}
      />
      <AiChatSheet onHeightChange={setSheetHeight} />
    </View>
  );

  function run(script: string) {
    webViewRef.current?.injectJavaScript(
      `(function () { ${script}; })(); true;`,
    );
  }
}

function withTimeout<T>(promise: Promise<T>, milliseconds: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("timeout")), milliseconds);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

function parseMessage(data: string): MapMessage | null {
  try {
    const value: unknown = JSON.parse(data);
    if (!value || typeof value !== "object" || !("type" in value)) {
      return null;
    }
    return value as MapMessage;
  } catch {
    return null;
  }
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: "#F2F2F7",
  },
  map: {
    ...StyleSheet.absoluteFill,
    backgroundColor: "#F2F2F7",
  },
  status: {
    ...StyleSheet.absoluteFill,
    alignItems: "center",
    justifyContent: "center",
    paddingHorizontal: 32,
    backgroundColor: "#F2F2F7",
  },
  statusText: {
    color: "#1C1C1E",
    fontSize: 18,
    lineHeight: 26,
    textAlign: "center",
  },
  callout: {
    position: "absolute",
    backgroundColor: "#FFFFFF",
    borderColor: "#1C1C1E",
    borderRadius: 16,
    borderWidth: 2,
    paddingHorizontal: 16,
    paddingVertical: 12,
  },
  calloutText: {
    color: "#1C1C1E",
    fontSize: 16,
    fontWeight: "600",
    lineHeight: 22,
  },
  locationButton: {
    position: "absolute",
    backgroundColor: "#1C1C1E",
    borderRadius: 16,
    paddingHorizontal: 16,
    paddingVertical: 12,
  },
  locationButtonText: {
    color: "#FFFFFF",
    fontSize: 16,
    fontWeight: "600",
    lineHeight: 22,
    textAlign: "center",
  },
});
