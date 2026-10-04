import { useEffect, useRef, useState, type RefObject } from "react";
import {
  AccessibilityInfo,
  Keyboard,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  useWindowDimensions,
  View,
} from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import {
  getTurn,
  listMessages,
  openChatStream,
  postMessage,
} from "../api/client";
import { ApiError, type ChatStreamEvent, type HistoryMessage } from "../api/types";

type AiChatSheetProps = {
  accessToken: string | null;
  authBusy: boolean;
  authError: string | null;
  onSignIn: (email: string, password: string) => void;
  onSignUp: (email: string, password: string) => void;
  onSessionLost: (message: string) => void;
  onToolsFinished: () => void;
  onHeightChange: (height: number) => void;
  onTopChange: (offsetFromBottom: number) => void;
  onExpandedWithKeyboardChange: (active: boolean) => void;
  toggleRef: RefObject<View | null>;
};

type ChatMessage = {
  id: string;
  role: "user" | "assistant";
  text: string;
  local?: boolean;
  pending?: boolean;
  turnId?: string;
};

const BAR_HEIGHTS = [7, 13, 19, 13, 7];
const CARD_PADDING_TOP = 4;
const POLL_INTERVAL_MS = 700;
const POLL_DEADLINE_MS = 180_000;
const THINKING_LINE = "Thinking...";
const FAILURE_LINE = "The reply failed.";
const TIMEOUT_LINE = "The reply did not arrive.";

export function AiChatSheet({
  accessToken,
  authBusy,
  authError,
  onSignIn,
  onSignUp,
  onSessionLost,
  onToolsFinished,
  onHeightChange,
  onTopChange,
  onExpandedWithKeyboardChange,
  toggleRef,
}: AiChatSheetProps) {
  const insets = useSafeAreaInsets();
  const { height: windowHeight } = useWindowDimensions();
  const transcriptRef = useRef<ScrollView>(null);
  const nextMessageId = useRef(0);
  const headerHeight = useRef(0);
  const composerHeight = useRef(0);
  const cardHeight = useRef(0);
  const mountedRef = useRef(true);
  const accessTokenRef = useRef(accessToken);
  const onSessionLostRef = useRef(onSessionLost);
  const onToolsFinishedRef = useRef(onToolsFinished);
  const pendingTurns = useRef(new Set<string>());
  const finishedTurns = useRef(new Set<string>());
  const repliedTurns = useRef(new Set<string>());
  const pollingTurns = useRef(new Set<string>());
  const onStreamEvent = useRef<(event: ChatStreamEvent) => void>(() => {});
  const onStreamError = useRef<(error: Error) => void>(() => {});
  const [expanded, setExpanded] = useState(true);
  const [draft, setDraft] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [authMode, setAuthMode] = useState<"sign-in" | "sign-up">("sign-in");
  const [authEmail, setAuthEmail] = useState("");
  const [authPassword, setAuthPassword] = useState("");
  const [authPasswordAgain, setAuthPasswordAgain] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [chatError, setChatError] = useState<string | null>(null);
  const [keyboardInset, setKeyboardInset] = useState(0);
  const [keyboardOpen, setKeyboardOpen] = useState(false);
  const [reduceMotion, setReduceMotion] = useState(false);
  const signedIn = accessToken != null;

  accessTokenRef.current = accessToken;
  onSessionLostRef.current = onSessionLost;
  onToolsFinishedRef.current = onToolsFinished;

  const halfScreen = Math.round(windowHeight * 0.5);
  const panelHeight = Math.min(halfScreen, windowHeight - keyboardInset);

  const finishTurn = (turnId: string) => {
    finishedTurns.current.add(turnId);
    pendingTurns.current.delete(turnId);
  };

  const dropMessage = (id: string) => {
    setMessages((current) => current.filter((message) => message.id !== id));
  };

  const writeAssistant = (turnId: string, text: string, mode: "append" | "replace") => {
    if (!mountedRef.current) {
      return;
    }
    const id = `assistant-${turnId}`;
    setMessages((current) => {
      let index = current.findIndex(
        (message) => message.role === "assistant" && message.turnId === turnId,
      );
      if (index === -1) {
        index = current.findIndex((message) => message.pending && message.turnId == null);
      }
      if (index === -1) {
        return [...current, { id, role: "assistant", text, turnId }];
      }
      const existing = current[index];
      if (!existing) {
        return current;
      }
      const base = existing.pending ? "" : existing.text;
      const next = current.slice();
      next[index] = {
        ...existing,
        id,
        local: false,
        pending: false,
        turnId,
        text: mode === "append" ? base + text : text,
      };
      return next;
    });
  };

  const pollTurn = (turnId: string) => {
    if (pollingTurns.current.has(turnId) || finishedTurns.current.has(turnId)) {
      return;
    }
    const token = accessTokenRef.current;
    if (!token) {
      return;
    }
    pollingTurns.current.add(turnId);
    const started = Date.now();
    void (async () => {
      let lastError: unknown = null;
      try {
        while (Date.now() - started < POLL_DEADLINE_MS) {
          if (
            !mountedRef.current ||
            accessTokenRef.current !== token ||
            finishedTurns.current.has(turnId)
          ) {
            return;
          }
          try {
            const turn = await getTurn(token, turnId);
            if (
              !mountedRef.current ||
              accessTokenRef.current !== token ||
              finishedTurns.current.has(turnId)
            ) {
              return;
            }
            lastError = null;
            if (turn.status === "done") {
              finishTurn(turnId);
              repliedTurns.current.add(turnId);
              writeAssistant(turnId, turn.reply_text ?? "", "replace");
              return;
            }
            if (turn.status === "failed") {
              finishTurn(turnId);
              writeAssistant(turnId, FAILURE_LINE, "replace");
              AccessibilityInfo.announceForAccessibility(FAILURE_LINE);
              return;
            }
          } catch (error) {
            if (
              !mountedRef.current ||
              accessTokenRef.current !== token ||
              finishedTurns.current.has(turnId)
            ) {
              return;
            }
            if (isUnauthorized(error)) {
              onSessionLostRef.current(readableError(error));
              return;
            }
            lastError = error;
          }
          await delay(POLL_INTERVAL_MS);
        }
        if (
          !mountedRef.current ||
          accessTokenRef.current !== token ||
          finishedTurns.current.has(turnId)
        ) {
          return;
        }
        finishTurn(turnId);
        if (repliedTurns.current.has(turnId)) {
          return;
        }
        const message = lastError == null ? TIMEOUT_LINE : readableError(lastError);
        writeAssistant(turnId, message, "replace");
        AccessibilityInfo.announceForAccessibility(message);
      } finally {
        pollingTurns.current.delete(turnId);
      }
    })();
  };

  onStreamEvent.current = (event) => {
    if (!mountedRef.current) {
      return;
    }
    switch (event.type) {
      case "reply.delta":
        if (finishedTurns.current.has(event.turn_id)) {
          return;
        }
        if (event.text.length > 0) {
          repliedTurns.current.add(event.turn_id);
        }
        writeAssistant(event.turn_id, event.text, "append");
        return;
      case "reply.done":
        finishTurn(event.turn_id);
        repliedTurns.current.add(event.turn_id);
        writeAssistant(event.turn_id, event.text, "replace");
        return;
      case "turn.failed":
        finishTurn(event.turn_id);
        writeAssistant(event.turn_id, event.error, "replace");
        AccessibilityInfo.announceForAccessibility(event.error);
        return;
      case "tool.finished":
        if (event.ok) {
          onToolsFinishedRef.current();
        }
        return;
      default:
        return;
    }
  };

  onStreamError.current = (error) => {
    if (!mountedRef.current) {
      return;
    }
    if (isUnauthorized(error)) {
      onSessionLostRef.current(readableError(error));
      return;
    }
    for (const turnId of [...pendingTurns.current]) {
      pollTurn(turnId);
    }
  };

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    let mounted = true;
    AccessibilityInfo.isReduceMotionEnabled().then((enabled) => {
      if (mounted) {
        setReduceMotion(enabled);
      }
    });
    const subscription = AccessibilityInfo.addEventListener(
      "reduceMotionChanged",
      setReduceMotion,
    );
    return () => {
      mounted = false;
      subscription.remove();
    };
  }, []);

  useEffect(() => {
    const showEvent =
      Platform.OS === "ios" ? "keyboardWillShow" : "keyboardDidShow";
    const hideEvent =
      Platform.OS === "ios" ? "keyboardWillHide" : "keyboardDidHide";
    const show = Keyboard.addListener(showEvent, (event) => {
      setKeyboardOpen(true);
      if (Platform.OS === "ios") {
        setKeyboardInset(event.endCoordinates.height);
      }
    });
    const hide = Keyboard.addListener(hideEvent, () => {
      setKeyboardOpen(false);
      if (Platform.OS === "ios") {
        setKeyboardInset(0);
      }
    });
    return () => {
      show.remove();
      hide.remove();
    };
  }, []);

  useEffect(() => {
    onExpandedWithKeyboardChange(expanded && keyboardOpen);
  }, [expanded, keyboardOpen, onExpandedWithKeyboardChange]);

  useEffect(() => {
    if (!expanded || !signedIn) {
      return;
    }
    const frame = requestAnimationFrame(() => {
      transcriptRef.current?.scrollToEnd({ animated: !reduceMotion });
    });
    return () => cancelAnimationFrame(frame);
  }, [expanded, messages, reduceMotion, signedIn]);

  useEffect(() => {
    pendingTurns.current.clear();
    finishedTurns.current.clear();
    repliedTurns.current.clear();
    if (!accessToken) {
      setMessages([]);
      setChatError(null);
      setDraft("");
      return;
    }
    const token = accessToken;
    let cancelled = false;
    void listMessages(token)
      .then((history) => {
        if (cancelled || !mountedRef.current) {
          return;
        }
        setMessages((current) => {
          const extras = current.filter((message) => message.local || message.turnId);
          return [...history.map(historyToMessage), ...extras];
        });
      })
      .catch((error: unknown) => {
        if (cancelled || !mountedRef.current) {
          return;
        }
        if (isUnauthorized(error)) {
          onSessionLostRef.current(readableError(error));
          return;
        }
        const message = readableError(error);
        setChatError(message);
        AccessibilityInfo.announceForAccessibility(message);
      });
    return () => {
      cancelled = true;
    };
  }, [accessToken]);

  useEffect(() => {
    if (!accessToken) {
      return;
    }
    const stream = openChatStream(
      accessToken,
      (event) => {
        onStreamEvent.current(event);
      },
      (error) => {
        onStreamError.current(error);
      },
    );
    return () => {
      stream.close();
    };
  }, [accessToken]);

  const toggleExpanded = () => {
    const next = !expanded;
    setExpanded(next);
    AccessibilityInfo.announceForAccessibility(
      next ? "Chat expanded" : "Chat minimized. Conversation hidden.",
    );
  };

  const sendDraft = () => {
    const token = accessToken;
    if (!token) {
      return;
    }
    const text = draft.trim();
    if (!text) {
      return;
    }
    nextMessageId.current += 1;
    const message: ChatMessage = {
      id: `local-${nextMessageId.current}`,
      role: "user",
      text,
      local: true,
    };
    const thinkingId = `thinking-${message.id}`;
    setMessages((current) => [
      ...current,
      message,
      {
        id: thinkingId,
        role: "assistant",
        text: THINKING_LINE,
        pending: true,
        local: true,
      },
    ]);
    setDraft("");
    setChatError(null);
    AccessibilityInfo.announceForAccessibility("Message sent. Thinking.");
    void deliverMessage(token, text, thinkingId);
  };

  async function deliverMessage(token: string, text: string, thinkingId: string) {
    try {
      const accepted = await postMessage(token, text);
      if (!mountedRef.current || accessTokenRef.current !== token) {
        return;
      }
      if (finishedTurns.current.has(accepted.turn_id)) {
        dropMessage(thinkingId);
        return;
      }
      const assistantId = `assistant-${accepted.turn_id}`;
      setMessages((current) =>
        current.map((message) =>
          message.id === thinkingId
            ? { ...message, id: assistantId, turnId: accepted.turn_id }
            : message,
        ),
      );
      pendingTurns.current.add(accepted.turn_id);
      pollTurn(accepted.turn_id);
    } catch (error) {
      if (!mountedRef.current || accessTokenRef.current !== token) {
        return;
      }
      dropMessage(thinkingId);
      if (isUnauthorized(error)) {
        onSessionLostRef.current(readableError(error));
        return;
      }
      const message = readableError(error);
      setChatError(message);
      AccessibilityInfo.announceForAccessibility(message);
    }
  }

  const awaitingReply = messages.some((message) => message.pending);
  const submitAuth = () => {
    const email = authEmail.trim();
    const password = authPassword;
    if (!email) {
      setFormError("Enter an email.");
      AccessibilityInfo.announceForAccessibility("Enter an email.");
      return;
    }
    if (authMode === "sign-up") {
      if (password !== authPasswordAgain) {
        setFormError("Hasła nie są takie same.");
        AccessibilityInfo.announceForAccessibility("Hasła nie są takie same.");
        return;
      }
      if (password.length < 8 || password.length > 128) {
        const message = "Password must be 8 to 128 characters.";
        setFormError(message);
        AccessibilityInfo.announceForAccessibility(message);
        return;
      }
      setFormError(null);
      onSignUp(email, password);
      return;
    }
    if (!password) {
      setFormError("Enter a password.");
      AccessibilityInfo.announceForAccessibility("Enter a password.");
      return;
    }
    setFormError(null);
    onSignIn(email, password);
  };

  const bottomInset = keyboardInset > 0 ? 12 : Math.max(insets.bottom, 12);

  const publishMapClearance = (header: number, composer: number) => {
    if (composer === 0) {
      return;
    }
    if (signedIn && header === 0) {
      return;
    }
    const headerHeightForInset = signedIn ? header : 0;
    onHeightChange(headerHeightForInset + composer + bottomInset + CARD_PADDING_TOP);
  };

  const publishTop = (height: number) => {
    cardHeight.current = height;
    onTopChange(height + keyboardInset);
  };

  useEffect(() => {
    publishMapClearance(headerHeight.current, composerHeight.current);
  }, [bottomInset, onHeightChange, signedIn]);

  useEffect(() => {
    if (cardHeight.current === 0) {
      return;
    }
    onTopChange(cardHeight.current + keyboardInset);
  }, [keyboardInset, onTopChange]);

  return (
    <View pointerEvents="box-none" style={[styles.dock, { bottom: keyboardInset }]}>
      <View
        onLayout={(event) => {
          publishTop(event.nativeEvent.layout.height);
        }}
        style={[
          styles.card,
          expanded && signedIn ? { height: panelHeight } : null,
          { paddingBottom: bottomInset },
        ]}
      >
        {signedIn ? (
          <View
            style={styles.header}
            onLayout={(event) => {
              headerHeight.current = event.nativeEvent.layout.height;
              publishMapClearance(headerHeight.current, composerHeight.current);
            }}
          >
            <Pressable
              ref={toggleRef}
              accessibilityRole="button"
              accessibilityLabel={expanded ? "Minimize chat" : "Expand chat"}
              accessibilityHint={
                expanded
                  ? "Hides the conversation. You can keep sending messages."
                  : "Shows the conversation."
              }
              accessibilityState={{ expanded }}
              onPress={toggleExpanded}
              hitSlop={4}
              style={({ pressed }) => [
                styles.chevronButton,
                pressed && styles.pressed,
              ]}
            >
              <ChevronGlyph expanded={expanded} />
            </Pressable>
          </View>
        ) : null}
        {expanded && signedIn ? (
          <ScrollView
            ref={transcriptRef}
            style={styles.transcript}
            contentContainerStyle={[
              styles.transcriptContent,
              messages.length === 0 && styles.transcriptEmpty,
            ]}
            keyboardShouldPersistTaps="handled"
            keyboardDismissMode="interactive"
            accessibilityLabel="Conversation"
          >
            {messages.length === 0 ? (
              <Text style={styles.empty}>
                Messages you send show up here.
              </Text>
            ) : (
              messages.map((message) => {
                const assistant = message.role === "assistant";
                return (
                  <View
                    key={message.id}
                    accessible
                    accessibilityRole="text"
                    accessibilityLabel={
                      message.pending
                        ? "Assistant is thinking"
                        : assistant
                          ? `Assistant said: ${message.text}`
                          : `You said: ${message.text}`
                    }
                    accessibilityLiveRegion={message.pending ? "polite" : undefined}
                    style={[styles.bubble, assistant && styles.assistantBubble]}
                  >
                    <Text
                      style={[
                        styles.bubbleText,
                        assistant && styles.assistantText,
                        message.pending && styles.pendingText,
                      ]}
                      importantForAccessibility="no"
                      accessibilityElementsHidden
                    >
                      {message.text}
                    </Text>
                  </View>
                );
              })
            )}
          </ScrollView>
        ) : null}
        <View
          onLayout={(event) => {
            composerHeight.current = event.nativeEvent.layout.height;
            publishMapClearance(headerHeight.current, composerHeight.current);
          }}
        >
          {signedIn ? (
            <>
              {chatError ? <Text style={styles.errorText}>{chatError}</Text> : null}
              {!expanded && awaitingReply ? (
                <Text
                  style={styles.pendingStatus}
                  accessibilityRole="text"
                  accessibilityLiveRegion="polite"
                >
                  {THINKING_LINE}
                </Text>
              ) : null}
              <Pressable
                accessibilityRole="button"
                accessibilityLabel="Tap to speak"
                accessibilityHint="Voice input is not available yet."
                onPress={() => {
                  AccessibilityInfo.announceForAccessibility(
                    "Voice input is not available yet.",
                  );
                }}
                style={({ pressed }) => [styles.speak, pressed && styles.pressed]}
              >
                <SoundWave />
                <Text
                  style={styles.speakLabel}
                  maxFontSizeMultiplier={1.8}
                  importantForAccessibility="no"
                  accessibilityElementsHidden
                >
                  Tap to speak
                </Text>
              </Pressable>
              <TextInput
                value={draft}
                onChangeText={setDraft}
                onSubmitEditing={sendDraft}
                placeholder="Message"
                placeholderTextColor="#8E8E93"
                returnKeyType="send"
                enablesReturnKeyAutomatically
                submitBehavior="submit"
                accessibilityLabel="Message"
                accessibilityHint="Sends your message. The conversation is kept when the chat is minimized."
                style={styles.input}
                maxFontSizeMultiplier={2}
              />
            </>
          ) : (
            <View style={styles.authActions}>
              {authError || formError ? (
                <Text style={styles.errorText} accessibilityRole="alert">
                  {formError ?? authError}
                </Text>
              ) : null}
              <Text style={styles.fieldLabel}>email</Text>
              <TextInput
                value={authEmail}
                onChangeText={setAuthEmail}
                editable={!authBusy}
                autoCapitalize="none"
                autoCorrect={false}
                autoComplete="email"
                keyboardType="email-address"
                textContentType="emailAddress"
                accessibilityLabel="email"
                style={styles.input}
                maxFontSizeMultiplier={2}
              />
              <Text style={styles.fieldLabel}>hasło</Text>
              <TextInput
                value={authPassword}
                onChangeText={setAuthPassword}
                editable={!authBusy}
                secureTextEntry
                autoCapitalize="none"
                autoCorrect={false}
                autoComplete={authMode === "sign-up" ? "new-password" : "password"}
                textContentType={authMode === "sign-up" ? "newPassword" : "password"}
                accessibilityLabel="hasło"
                style={styles.input}
                maxFontSizeMultiplier={2}
              />
              {authMode === "sign-up" ? (
                <>
                  <Text style={styles.fieldLabel}>powtórz hasło</Text>
                  <TextInput
                    value={authPasswordAgain}
                    onChangeText={setAuthPasswordAgain}
                    editable={!authBusy}
                    secureTextEntry
                    autoCapitalize="none"
                    autoCorrect={false}
                    autoComplete="new-password"
                    textContentType="newPassword"
                    accessibilityLabel="powtórz hasło"
                    style={styles.input}
                    maxFontSizeMultiplier={2}
                  />
                </>
              ) : null}
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={authMode === "sign-up" ? "Sign Up" : "Sign In"}
                accessibilityState={{ disabled: authBusy, busy: authBusy }}
                disabled={authBusy}
                onPress={submitAuth}
                style={({ pressed }) => [
                  styles.authButton,
                  authBusy && styles.authButtonDisabled,
                  pressed && !authBusy && styles.pressed,
                ]}
              >
                <Text
                  style={styles.authLabel}
                  maxFontSizeMultiplier={1.8}
                  importantForAccessibility="no"
                  accessibilityElementsHidden
                >
                  {authMode === "sign-up" ? "Sign Up" : "Sign In"}
                </Text>
              </Pressable>
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={
                  authMode === "sign-up"
                    ? "Already have an account? Sign in"
                    : "Need an account? Sign up"
                }
                disabled={authBusy}
                onPress={() => {
                  setAuthMode((current) => (current === "sign-in" ? "sign-up" : "sign-in"));
                  setAuthPassword("");
                  setAuthPasswordAgain("");
                  setFormError(null);
                }}
                style={styles.authSwitch}
              >
                <Text style={styles.authSwitchLabel} maxFontSizeMultiplier={1.8}>
                  {authMode === "sign-up"
                    ? "Already have an account? Sign in"
                    : "Need an account? Sign up"}
                </Text>
              </Pressable>
            </View>
          )}
        </View>
      </View>
    </View>
  );
}

function historyToMessage(message: HistoryMessage): ChatMessage {
  return {
    id: message.id,
    role: message.role,
    text: message.body,
  };
}

function readableError(error: unknown): string {
  if (error instanceof ApiError || error instanceof Error) {
    return error.message;
  }
  return "Something went wrong.";
}

function isUnauthorized(error: unknown): boolean {
  return error instanceof ApiError && error.status === 401;
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, milliseconds);
  });
}

function ChevronGlyph({ expanded }: { expanded: boolean }) {
  return (
    <View
      pointerEvents="none"
      importantForAccessibility="no"
      accessibilityElementsHidden
      style={[styles.chevron, expanded ? styles.chevronDown : styles.chevronUp]}
    />
  );
}

function SoundWave() {
  return (
    <View
      style={styles.wave}
      importantForAccessibility="no"
      accessibilityElementsHidden
    >
      {BAR_HEIGHTS.map((height, index) => (
        <View key={`bar-${index}`} style={[styles.bar, { height }]} />
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  dock: {
    position: "absolute",
    start: 0,
    end: 0,
    bottom: 0,
  },
  card: {
    backgroundColor: "#FFFFFF",
    borderColor: "#3A3A3C",
    borderTopLeftRadius: 24,
    borderTopRightRadius: 24,
    borderWidth: 1,
    borderBottomWidth: 0,
    paddingHorizontal: 16,
    paddingTop: CARD_PADDING_TOP,
    elevation: 8,
    shadowColor: "#000000",
    shadowOffset: { width: 0, height: -2 },
    shadowOpacity: 0.16,
    shadowRadius: 12,
  },
  header: {
    flexDirection: "row",
    justifyContent: "flex-end",
    alignItems: "center",
  },
  chevronButton: {
    width: 44,
    height: 44,
    alignItems: "center",
    justifyContent: "center",
    borderRadius: 22,
  },
  chevron: {
    width: 12,
    height: 12,
    borderBottomWidth: 2.5,
    borderRightWidth: 2.5,
    borderColor: "#1C1C1E",
  },
  chevronUp: {
    transform: [{ translateY: 3 }, { rotate: "-135deg" }],
  },
  chevronDown: {
    transform: [{ translateY: -2 }, { rotate: "45deg" }],
  },
  transcript: {
    flex: 1,
  },
  transcriptContent: {
    flexGrow: 1,
    justifyContent: "flex-end",
    gap: 8,
    paddingBottom: 12,
  },
  transcriptEmpty: {
    justifyContent: "center",
  },
  empty: {
    color: "#3A3A3C",
    fontSize: 16,
    lineHeight: 22,
    textAlign: "center",
    paddingHorizontal: 12,
  },
  bubble: {
    alignSelf: "flex-end",
    maxWidth: "85%",
    backgroundColor: "#1C1C1E",
    borderRadius: 16,
    paddingHorizontal: 14,
    paddingVertical: 10,
  },
  assistantBubble: {
    alignSelf: "flex-start",
    backgroundColor: "#F2F2F7",
  },
  bubbleText: {
    color: "#FFFFFF",
    fontSize: 16,
    lineHeight: 22,
  },
  assistantText: {
    color: "#1C1C1E",
  },
  pendingText: {
    color: "#8E8E93",
    fontStyle: "italic",
  },
  pendingStatus: {
    color: "#8E8E93",
    fontSize: 16,
    fontStyle: "italic",
    lineHeight: 22,
    textAlign: "center",
    marginBottom: 8,
  },
  speak: {
    minHeight: 52,
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "center",
    gap: 12,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: "#3A3A3C",
    backgroundColor: "#F2F2F7",
    paddingHorizontal: 16,
    marginBottom: 10,
  },
  speakLabel: {
    color: "#1C1C1E",
    fontSize: 17,
    fontWeight: "600",
    lineHeight: 22,
  },
  wave: {
    width: 28,
    height: 22,
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
  },
  bar: {
    width: 3,
    borderRadius: 1.5,
    backgroundColor: "#1C1C1E",
  },
  input: {
    minHeight: 48,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: "#3A3A3C",
    paddingHorizontal: 16,
    paddingVertical: 12,
    color: "#1C1C1E",
    fontSize: 17,
    lineHeight: 22,
  },
  authActions: {
    gap: 10,
  },
  fieldLabel: {
    color: "#1C1C1E",
    fontSize: 15,
    lineHeight: 20,
    fontWeight: "600",
  },
  authButton: {
    minHeight: 52,
    alignItems: "center",
    justifyContent: "center",
    borderRadius: 16,
    borderWidth: 1,
    borderColor: "#3A3A3C",
    backgroundColor: "#FFFFFF",
    paddingHorizontal: 16,
  },
  authButtonDisabled: {
    opacity: 0.4,
  },
  authLabel: {
    color: "#1C1C1E",
    fontSize: 17,
    fontWeight: "600",
    lineHeight: 22,
  },
  authSwitch: {
    minHeight: 44,
    alignItems: "center",
    justifyContent: "center",
  },
  authSwitchLabel: {
    color: "#3A3A3C",
    fontSize: 15,
    lineHeight: 20,
    textAlign: "center",
  },
  errorText: {
    color: "#1C1C1E",
    fontSize: 16,
    lineHeight: 22,
    textAlign: "center",
  },
  pressed: {
    backgroundColor: "#E5E5EA",
  },
});
