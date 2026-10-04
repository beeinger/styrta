import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AccessibilityInfo,
  ActivityIndicator,
  I18nManager,
  Keyboard,
  Platform,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from "react-native";
import { BlurTargetView, BlurView } from "expo-blur";
import * as Location from "expo-location";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { WebView, type WebViewHandle, type WebViewMessageEvent } from "./MapWebView";

import { getMyEvents, getNearbyEvents, signIn as requestSignIn, signUp as requestSignUp } from "../api/client";
import { ApiError, type MyEvent, type NearbyEvent, type Session } from "../api/types";
import { describeAttendance, formatEventStart, type MeetupEvent } from "../data/events";
import { chatCornerRadius, space } from "../theme";
import { AiChatSheet } from "./AiChatSheet";
import { AttendingBubbles } from "./AttendingBubbles";
import { createMapHtml, type MapMarker, type MapPadding } from "./mapDocument";
import { MapZoomControls } from "./MapZoomControls";

const ARENA_CAMERA = {
  latitude: 50.0683,
  longitude: 19.9917,
  zoom: 14,
};

const FALLBACK_CAMERA = ARENA_CAMERA;

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
const MAX_ZOOM = 15;
const FETCH_DEBOUNCE_MS = 300;

type Viewport = {
  latitude: number;
  longitude: number;
  zoom: number;
  bbox: string;
};

type NearbyQuery = {
  lat: number;
  lng: number;
  zoom: number;
  bbox?: string;
};

type MapMessage =
  | { type: "ready"; zoom: number }
  | { type: "zoom"; zoom: number; source?: "control" | "gesture" }
  | { type: "viewport"; latitude: number; longitude: number; zoom: number; bbox: string }
  | { type: "zoom-blocked"; direction: "in" | "out" }
  | { type: "marker-press"; id: string }
  | { type: "map-press" }
  | { type: "error"; message?: string };

export function MapScreen() {
  const insets = useSafeAreaInsets();
  const webViewRef = useRef<WebViewHandle>(null);
  const chatToggleRef = useRef<View>(null);
  const blurTargetRef = useRef<View>(null);
  const announcedReady = useRef(false);
  const announcedLocation = useRef(false);
  const reduceMotionRef = useRef(false);
  const [startCamera, setStartCamera] = useState<Camera | null>(ARENA_CAMERA);
  const [userLocation, setUserLocation] = useState<Coordinates | null>(null);
  const [zoom, setZoom] = useState(FALLBACK_CAMERA.zoom);
  const [sheetHeight, setSheetHeight] = useState(0);
  const [chatTop, setChatTop] = useState(0);
  const [mapReady, setMapReady] = useState(false);
  const [mapFailed, setMapFailed] = useState(false);
  const [reduceMotion, setReduceMotion] = useState(false);
  const [selectedMarker, setSelectedMarker] = useState<MapMarker | null>(null);
  const [eventsOpen, setEventsOpen] = useState(false);
  const [chatCoversEvents, setChatCoversEvents] = useState(false);
  const [session, setSession] = useState<Session | null>(null);
  const [authBusy, setAuthBusy] = useState(false);
  const [authError, setAuthError] = useState<string | null>(null);
  const [attending, setAttending] = useState<MeetupEvent[]>([]);
  const [markers, setMarkers] = useState<MapMarker[]>([]);
  const [viewport, setViewport] = useState<Viewport | null>(null);
  const [eventsVersion, setEventsVersion] = useState(0);
  const locating = useRef(false);
  const mapFrameHeight = useRef(0);
  const mapReadyRef = useRef(false);
  const aliveRef = useRef(true);
  const authLock = useRef(false);
  const sessionRef = useRef<Session | null>(null);
  const markersRef = useRef<MapMarker[]>([]);
  const nearbyQueryRef = useRef<NearbyQuery>({
    lat: ARENA_CAMERA.latitude,
    lng: ARENA_CAMERA.longitude,
    zoom: ARENA_CAMERA.zoom,
  });
  mapReadyRef.current = mapReady;
  markersRef.current = markers;
  sessionRef.current = session;
  const nearbyQuery = nearbyQueryFor(viewport, userLocation);
  const nearbyKey = nearbyKeyFor(viewport, userLocation);
  nearbyQueryRef.current = nearbyQuery;
  const obstruction = sheetHeight || 168;
  const mapBottom = chatTop || obstruction;

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

  const centerOnUser = useCallback(
    async (recenter: boolean, announce: boolean) => {
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
          if (recenter) {
            AccessibilityInfo.announceForAccessibility(
              "Location access was denied.",
            );
          }
          return;
        }

        if (Platform.OS === "android") {
          try {
            await Location.enableNetworkProviderAsync();
          } catch {
            // The position request below fails if location services stay off.
          }
        }

        let latest: Coordinates | null = null;
        const lastKnown = await Location.getLastKnownPositionAsync();
        if (lastKnown) {
          latest = {
            latitude: lastKnown.coords.latitude,
            longitude: lastKnown.coords.longitude,
          };
          publishLocation(lastKnown.coords);
        }

        try {
          const current = await withTimeout(
            Location.getCurrentPositionAsync({
              accuracy: Location.Accuracy.Balanced,
            }),
            10000,
          );
          latest = {
            latitude: current.coords.latitude,
            longitude: current.coords.longitude,
          };
          publishLocation(current.coords);
        } catch {
          if (!latest && recenter) {
            AccessibilityInfo.announceForAccessibility(
              "Could not find your location.",
            );
          }
        }

        if (recenter && latest) {
          moveMapTo(latest, announce);
        }
      } finally {
        locating.current = false;
      }
    },
    [],
  );

  function publishLocation(coords: Location.LocationObjectCoords) {
    const next = {
      latitude: coords.latitude,
      longitude: coords.longitude,
    };
    setUserLocation(next);
    setStartCamera((current) => current ?? { ...next, zoom: USER_ZOOM });
    if (!announcedLocation.current) {
      announcedLocation.current = true;
      AccessibilityInfo.announceForAccessibility("Showing your location");
    }
  }

  function moveMapTo(coords: Coordinates, announce: boolean) {
    if (!mapReadyRef.current) {
      return;
    }
    run(
      `window.__styrtaMap.centerOn(${coords.latitude}, ${coords.longitude}, ${reduceMotionRef.current ? "false" : "true"})`,
    );
    setZoom((currentZoom) => Math.max(currentZoom, USER_ZOOM));
    if (announce) {
      AccessibilityInfo.announceForAccessibility("Centered on your location");
    }
  }

  const onSessionLost = useCallback((message: string) => {
    if (sessionRef.current == null) {
      return;
    }
    sessionRef.current = null;
    setSession(null);
    setAttending([]);
    setMarkers([]);
    setSelectedMarker(null);
    setAuthError(message);
    AccessibilityInfo.announceForAccessibility(message);
  }, []);

  const refreshEvents = useCallback(() => {
    setEventsVersion((current) => current + 1);
  }, []);

  const runAuth = useCallback(
    async (action: "sign-in" | "sign-up", email: string, password: string) => {
      if (authLock.current) {
        return;
      }
      authLock.current = true;
      setAuthBusy(true);
      setAuthError(null);
      AccessibilityInfo.announceForAccessibility(
        action === "sign-in" ? "Signing in" : "Signing up",
      );
      try {
        const next =
          action === "sign-in"
            ? await requestSignIn(email, password)
            : await requestSignUp(email, password);
        if (!aliveRef.current) {
          return;
        }
        setSession(next);
        AccessibilityInfo.announceForAccessibility("Signed in");
      } catch (error) {
        if (!aliveRef.current) {
          return;
        }
        const message = authErrorMessage(error);
        setAuthError(message);
        AccessibilityInfo.announceForAccessibility(message);
      } finally {
        authLock.current = false;
        if (aliveRef.current) {
          setAuthBusy(false);
        }
      }
    },
    [],
  );

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
    };
  }, []);

  useEffect(() => {
    let mounted = true;

    void centerOnUser(false, false);

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

  const endInset = I18nManager.isRTL ? insets.left : insets.right;
  const startInset = I18nManager.isRTL ? insets.right : insets.left;
  const endClearance = Math.max(16, endInset) + 64;
  const startClearance = Math.max(16, startInset) + 64;

  const mapPadding = useMemo<MapPadding>(
    () => ({
      top: insets.top,
      // The map draws this far under the chat so the rounded corners show
      // tiles. The same inset keeps the visual center where it was.
      bottom: chatCornerRadius,
      left: I18nManager.isRTL ? endClearance : startClearance,
      right: I18nManager.isRTL ? startClearance : endClearance,
    }),
    [endClearance, insets.top, startClearance],
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
  }, [mapReady, markers]);

  useEffect(() => {
    if (!session) {
      return;
    }
    const token = session.access_token;
    let cancelled = false;
    const timer = setTimeout(() => {
      void getMyEvents(token)
        .then((events) => {
          if (cancelled) {
            return;
          }
          setAttending(events.map((event) => myEventToMeetup(event, session.id)));
        })
        .catch((error: unknown) => {
          if (cancelled) {
            return;
          }
          if (isUnauthorized(error)) {
            onSessionLost(readableError(error));
          }
        });
    }, FETCH_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [session, eventsVersion, onSessionLost]);

  useEffect(() => {
    if (!session) {
      return;
    }
    const token = session.access_token;
    let cancelled = false;
    const timer = setTimeout(() => {
      const query = nearbyQueryRef.current;
      void getNearbyEvents(token, query)
        .then((events) => {
          if (cancelled) {
            return;
          }
          setMarkers(events.map(nearbyToMarker));
        })
        .catch((error: unknown) => {
          if (cancelled) {
            return;
          }
          if (isUnauthorized(error)) {
            onSessionLost(readableError(error));
          }
        });
    }, FETCH_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [session, nearbyKey, eventsVersion, onSessionLost]);

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
      `window.__styrtaMap.setUserLocation(${userLocation.latitude}, ${userLocation.longitude})`,
    );
  }, [mapReady, userLocation]);

  useEffect(() => {
    if (!startCamera || mapReady || mapFailed) {
      return;
    }

    const timer = setTimeout(() => setMapFailed(true), 15000);
    return () => clearTimeout(timer);
  }, [mapFailed, mapReady, startCamera]);

  const focusEvent = (event: MeetupEvent) => {
    const marker = markers.find((item) => item.id === event.id);
    if (marker) {
      setSelectedMarker(marker);
    }
    if (!mapReady) {
      return;
    }
    run(
      `window.__styrtaMap.centerOn(${event.latitude}, ${event.longitude}, ${reduceMotion ? "false" : "true"})`,
    );
    AccessibilityInfo.announceForAccessibility(`Showing ${event.title}`);
  };

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

    if (message.type === "viewport") {
      setZoom(message.zoom);
      setViewport({
        latitude: message.latitude,
        longitude: message.longitude,
        zoom: message.zoom,
        bbox: message.bbox,
      });
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

    if (message.type === "marker-press" || message.type === "map-press") {
      Keyboard.dismiss();
    }

    if (message.type === "marker-press") {
      const marker = markersRef.current.find((item) => item.id === message.id);
      if (!marker) {
        return;
      }
      setSelectedMarker(marker);
      run(
        `window.__styrtaMap.centerOn(${marker.latitude}, ${marker.longitude}, ${reduceMotion ? "false" : "true"})`,
      );
      AccessibilityInfo.announceForAccessibility(
        `${marker.title}, ${marker.startsAtLabel}, hosted by ${marker.hostName}, ${describeAttendance(marker.signedCount, marker.capacity)}`,
      );
      return;
    }

    if (message.type === "map-press") {
      setSelectedMarker(null);
      return;
    }

    setMapFailed(true);
  };

  return (
    <View
      style={styles.screen}
      onStartShouldSetResponderCapture={(event) => {
        const focused = TextInput.State.currentlyFocusedInput();
        if (
          focused == null ||
          event.target === focused ||
          event.target === chatToggleRef.current
        ) {
          return false;
        }
        Keyboard.dismiss();
        return false;
      }}
    >
      <BlurTargetView
        ref={blurTargetRef}
        style={StyleSheet.absoluteFill}
        pointerEvents={eventsOpen && !chatCoversEvents ? "none" : "auto"}
      >
      {html ? (
        <WebView
          ref={webViewRef}
          source={{ html, baseUrl: "https://localhost" }}
          style={[
            styles.map,
            { bottom: Math.max(0, mapBottom - chatCornerRadius) },
          ]}
          onLayout={(event) => {
            const next = Math.round(event.nativeEvent.layout.height);
            if (next === mapFrameHeight.current) {
              return;
            }
            mapFrameHeight.current = next;
            if (!mapReadyRef.current) {
              return;
            }
            run(
              `window.__styrtaMap.resize(${reduceMotionRef.current ? "false" : "true"})`,
            );
          }}
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
      <MapZoomControls
        bottom={mapBottom + 12}
        canZoomIn={mapReady && zoom < MAX_ZOOM - 0.01}
        canZoomOut={mapReady && zoom > MIN_ZOOM + 0.01}
        canCenter={mapReady}
        onZoomIn={() => zoomBy("in")}
        onZoomOut={() => zoomBy("out")}
        onCenter={() => {
          if (userLocation) {
            moveMapTo(userLocation, true);
            void centerOnUser(true, false);
            return;
          }
          void centerOnUser(true, true);
        }}
      />
      </BlurTargetView>
      {eventsOpen && !chatCoversEvents ? (
        <>
          <BlurView
            blurTarget={blurTargetRef}
            intensity={50}
            tint="light"
            blurMethod="dimezisBlurView"
            pointerEvents="none"
            style={[styles.mapOverlay, { bottom: mapBottom }]}
          />
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Close your events"
            onPress={() => {
              setEventsOpen(false);
              AccessibilityInfo.announceForAccessibility("Events closed");
            }}
            style={[styles.mapOverlay, { bottom: mapBottom }]}
          />
        </>
      ) : null}
      <AiChatSheet
        accessToken={session?.access_token ?? null}
        authBusy={authBusy}
        authError={authError}
        onSignIn={(email, password) => {
          void runAuth("sign-in", email, password);
        }}
        onSignUp={(email, password) => {
          void runAuth("sign-up", email, password);
        }}
        onSessionLost={onSessionLost}
        onToolsFinished={refreshEvents}
        onHeightChange={setSheetHeight}
        onTopChange={setChatTop}
        onExpandedWithKeyboardChange={setChatCoversEvents}
        toggleRef={chatToggleRef}
      />
      <AttendingBubbles
        events={session ? attending : []}
        expanded={eventsOpen}
        concealed={chatCoversEvents}
        onExpandedChange={setEventsOpen}
        top={insets.top + space.md}
        bottom={mapBottom + space.sm}
        start={Math.max(space.sm * 2, startInset) / 2}
        end={space.sm}
        onFocusEvent={focusEvent}
      />
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
    const record = value as Record<string, unknown>;
    if (record.type === "viewport") {
      if (
        typeof record.latitude !== "number" ||
        typeof record.longitude !== "number" ||
        typeof record.zoom !== "number" ||
        typeof record.bbox !== "string" ||
        !Number.isFinite(record.latitude) ||
        !Number.isFinite(record.longitude) ||
        !Number.isFinite(record.zoom)
      ) {
        return null;
      }
      return {
        type: "viewport",
        latitude: record.latitude,
        longitude: record.longitude,
        zoom: record.zoom,
        bbox: record.bbox,
      };
    }
    return value as MapMessage;
  } catch {
    return null;
  }
}

function nearbyQueryFor(viewport: Viewport | null, userLocation: Coordinates | null): NearbyQuery {
  if (viewport) {
    return {
      lat: viewport.latitude,
      lng: viewport.longitude,
      zoom: viewport.zoom,
      bbox: viewport.bbox,
    };
  }
  if (userLocation) {
    return {
      lat: userLocation.latitude,
      lng: userLocation.longitude,
      zoom: USER_ZOOM,
    };
  }
  return {
    lat: ARENA_CAMERA.latitude,
    lng: ARENA_CAMERA.longitude,
    zoom: ARENA_CAMERA.zoom,
  };
}

function nearbyKeyFor(viewport: Viewport | null, userLocation: Coordinates | null): string {
  if (viewport) {
    return `${viewport.latitude},${viewport.longitude},${viewport.zoom},${viewport.bbox}`;
  }
  if (userLocation) {
    return `user:${userLocation.latitude},${userLocation.longitude}`;
  }
  return "arena";
}

function myEventToMeetup(event: MyEvent, userId: string): MeetupEvent {
  return {
    id: event.id,
    emoji: event.emoji,
    title: event.title,
    signedCount: event.signed_count,
    capacity: event.capacity,
    latitude: event.latitude,
    longitude: event.longitude,
    hostName: event.host_name,
    startsAt: event.starts_at,
    hostedByMe: event.host_id === userId,
  };
}

function nearbyToMarker(event: NearbyEvent): MapMarker {
  return {
    id: event.id,
    latitude: event.latitude,
    longitude: event.longitude,
    title: event.title,
    emoji: event.emoji,
    hostName: event.host_name,
    signedCount: event.signed_count,
    capacity: event.capacity,
    startsAtLabel: formatEventStart(event.starts_at),
  };
}

function readableError(error: unknown): string {
  if (error instanceof ApiError || error instanceof Error) {
    return error.message;
  }
  return "Something went wrong.";
}

function authErrorMessage(error: unknown): string {
  switch (readableError(error)) {
    case "email":
      return "Enter a valid email.";
    case "password":
      return "Password must be 8 to 128 characters.";
    case "email taken":
      return "That email is already registered.";
    case "unauthorized":
      return "Email or password is wrong.";
    default:
      return readableError(error);
  }
}

function isUnauthorized(error: unknown): boolean {
  return error instanceof ApiError && error.status === 401;
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: "#F2F2F7",
  },
  mapOverlay: {
    position: "absolute",
    top: 0,
    start: 0,
    end: 0,
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
});
