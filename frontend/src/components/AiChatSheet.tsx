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
import {
  AudioModule,
  RecordingPresets,
  createAudioPlayer,
  setAudioModeAsync,
  type AudioPlayer,
  type AudioRecorder,
} from "expo-audio";
import * as Clipboard from "expo-clipboard";
import * as Linking from "expo-linking";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import {
  getTurn,
  listMessages,
  openChatStream,
  postAudioMessage,
  postMessage,
  spokenReplySource,
  type SpeechClip,
} from "../api/client";
import { ApiError, type ChatStreamEvent, type HistoryMessage } from "../api/types";
import { chatCornerRadius, colors, fonts, space } from "../theme";

type AiChatSheetProps = {
  accessToken: string | null;
  userNick: string;
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
const CARD_PADDING_TOP = space.xs;
const POLL_INTERVAL_MS = 700;
const POLL_DEADLINE_MS = 180_000;
const THINKING_LINE = "Thinking...";
const LISTENING_LINE = "Listening…";
const FAILURE_LINE = "The reply failed.";
const TIMEOUT_LINE = "The reply did not arrive.";
const MIC_DENIED = "Microphone access is off. Allow it to speak.";
const CLIP_EMPTY = "No speech was captured. Tap to speak and try again.";
const MIN_CLIP_MS = 400;

export function AiChatSheet({
  accessToken,
  userNick,
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
  const replyPlayer = useRef<AudioPlayer | null>(null);
  const spokenTurns = useRef(new Set<string>());
  const announcedTranscripts = useRef(new Set<string>());
  const voiceLock = useRef(false);
  const voiceMutedRef = useRef(false);
  const recorderRef = useRef<AudioRecorder | null>(null);
  const [expanded, setExpanded] = useState(true);
  const [listening, setListening] = useState(false);
  const [sendingVoice, setSendingVoice] = useState(false);
  const [voiceMuted, setVoiceMuted] = useState(false);
  const [draft, setDraft] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [historyReady, setHistoryReady] = useState(false);
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

  const revealTranscript = (turnId: string, text: string) => {
    if (!mountedRef.current) {
      return;
    }
    let applied = false;
    setMessages((current) => {
      let matched = false;
      const next = current.map((message) => {
        if (message.role === "user" && message.turnId === turnId) {
          matched = true;
          return { ...message, text, pending: false };
        }
        return message;
      });
      if (matched) {
        applied = true;
        return next;
      }
      let index = -1;
      for (let cursor = next.length - 1; cursor >= 0; cursor -= 1) {
        const message = next[cursor];
        if (message?.role === "user" && message.pending) {
          index = cursor;
          break;
        }
      }
      const existing = index === -1 ? undefined : next[index];
      if (!existing) {
        return next;
      }
      applied = true;
      const copy = next.slice();
      copy[index] = { ...existing, turnId, text, pending: false };
      return copy;
    });
    if (applied && !announcedTranscripts.current.has(turnId)) {
      announcedTranscripts.current.add(turnId);
      AccessibilityInfo.announceForAccessibility(`You said: ${text}`);
    }
  };

  const dropPendingVoice = (turnId: string) => {
    setMessages((current) =>
      current.filter(
        (message) =>
          !(
            message.role === "user" &&
            message.pending &&
            (message.turnId === turnId || message.turnId == null)
          ),
      ),
    );
  };

  const playReply = async (turnId: string, url: string) => {
    if (spokenTurns.current.has(turnId) || voiceMutedRef.current) {
      spokenTurns.current.add(turnId);
      return;
    }
    const token = accessTokenRef.current;
    if (!token || !mountedRef.current) {
      return;
    }
    spokenTurns.current.add(turnId);
    try {
      await setAudioModeAsync(audioSession(false));
      if (!mountedRef.current || accessTokenRef.current !== token) {
        spokenTurns.current.delete(turnId);
        return;
      }
      replyPlayer.current?.release();
      const player = createAudioPlayer(spokenReplySource(token, url));
      replyPlayer.current = player;
      player.play();
    } catch {
      spokenTurns.current.delete(turnId);
      replyPlayer.current = null;
    }
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
              if (turn.user_text) {
                revealTranscript(turnId, turn.user_text);
              }
              writeAssistant(turnId, turn.reply_text ?? "", "replace");
              if (turn.audio_ready) {
                void playReply(turnId, `/v1/chat/turns/${turnId}/audio`);
              }
              return;
            }
            if (turn.status === "failed") {
              finishTurn(turnId);
              dropPendingVoice(turnId);
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
      case "transcript.ready":
        revealTranscript(event.turn_id, event.text);
        return;
      case "reply.done":
        finishTurn(event.turn_id);
        repliedTurns.current.add(event.turn_id);
        writeAssistant(event.turn_id, event.text, "replace");
        return;
      case "audio.ready":
        void playReply(event.turn_id, event.url);
        return;
      case "turn.failed":
        finishTurn(event.turn_id);
        dropPendingVoice(event.turn_id);
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
      replyPlayer.current?.release();
      replyPlayer.current = null;
    };
  }, []);

  useEffect(() => {
    return () => {
      const recorder = recorderRef.current;
      recorderRef.current = null;
      if (!recorder) {
        return;
      }
      try {
        recorder.release();
      } catch {
        // Already released.
      }
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
    spokenTurns.current.clear();
    announcedTranscripts.current.clear();
    replyPlayer.current?.release();
    replyPlayer.current = null;
    if (!accessToken) {
      setMessages([]);
      setHistoryReady(false);
      setChatError(null);
      setDraft("");
      setListening(false);
      setSendingVoice(false);
      releaseRecorder();
      return;
    }
    const token = accessToken;
    let cancelled = false;
    setHistoryReady(false);
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
      })
      .finally(() => {
        if (!cancelled && mountedRef.current) {
          setHistoryReady(true);
        }
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

  const toggleVoice = () => {
    const next = !voiceMutedRef.current;
    voiceMutedRef.current = next;
    setVoiceMuted(next);
    if (next) {
      try {
        replyPlayer.current?.pause();
      } catch {
        replyPlayer.current = null;
      }
    }
    AccessibilityInfo.announceForAccessibility(
      next ? "Assistant voice muted." : "Assistant voice on.",
    );
  };

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

  async function deliverAudio(token: string, clip: SpeechClip) {
    nextMessageId.current += 1;
    const messageId = `local-${nextMessageId.current}`;
    const thinkingId = `thinking-${messageId}`;
    setMessages((current) => [
      ...current,
      {
        id: messageId,
        role: "user",
        text: LISTENING_LINE,
        local: true,
        pending: true,
      },
      {
        id: thinkingId,
        role: "assistant",
        text: THINKING_LINE,
        pending: true,
        local: true,
      },
    ]);
    setChatError(null);
    AccessibilityInfo.announceForAccessibility("Message sent. Thinking.");
    try {
      const accepted = await postAudioMessage(token, clip);
      if (!mountedRef.current || accessTokenRef.current !== token) {
        return;
      }
      if (finishedTurns.current.has(accepted.turn_id)) {
        dropMessage(thinkingId);
        return;
      }
      const assistantId = `assistant-${accepted.turn_id}`;
      setMessages((current) =>
        current.map((message) => {
          if (message.id === messageId) {
            return { ...message, turnId: accepted.turn_id };
          }
          if (message.id === thinkingId) {
            return { ...message, id: assistantId, turnId: accepted.turn_id };
          }
          return message;
        }),
      );
      pendingTurns.current.add(accepted.turn_id);
      pollTurn(accepted.turn_id);
    } catch (error) {
      if (!mountedRef.current || accessTokenRef.current !== token) {
        return;
      }
      dropMessage(thinkingId);
      dropMessage(messageId);
      if (isUnauthorized(error)) {
        onSessionLostRef.current(readableError(error));
        return;
      }
      const message = readableError(error);
      setChatError(message);
      AccessibilityInfo.announceForAccessibility(message);
    }
  }

  function releaseRecorder() {
    const recorder = recorderRef.current;
    recorderRef.current = null;
    if (!recorder) {
      return;
    }
    try {
      recorder.release();
    } catch {
      // Already released.
    }
  }

  function ensureRecorder(): AudioRecorder {
    const existing = recorderRef.current;
    if (existing) {
      return existing;
    }
    const created = createRecorder();
    recorderRef.current = created;
    return created;
  }

  const onSpeak = () => {
    if (voiceLock.current || sendingVoice) {
      return;
    }
    const token = accessToken;
    if (!token) {
      return;
    }
    const recorder = ensureRecorder();
    if (listening) {
      voiceLock.current = true;
      setListening(false);
      setSendingVoice(true);
      void (async () => {
        try {
          const duration = recorder.getStatus().durationMillis;
          const stopped = (await recorder.stop()) as unknown as { url?: string | null };
          const stoppedUrl = stopped?.url;
          const uri =
            typeof stoppedUrl === "string" && stoppedUrl.length > 0
              ? stoppedUrl
              : recorder.uri;
          if (!mountedRef.current || accessTokenRef.current !== token) {
            return;
          }
          if (!uri || duration < MIN_CLIP_MS) {
            setChatError(CLIP_EMPTY);
            AccessibilityInfo.announceForAccessibility(CLIP_EMPTY);
            return;
          }
          await deliverAudio(token, speechClip(uri));
        } catch (error) {
          if (recorderReleased(error)) {
            recorderRef.current = null;
          }
          if (!mountedRef.current) {
            return;
          }
          const message = readableError(error);
          setChatError(message);
          AccessibilityInfo.announceForAccessibility(message);
        } finally {
          setSendingVoice(false);
          voiceLock.current = false;
        }
      })();
      return;
    }
    voiceLock.current = true;
    void (async () => {
      try {
        const permission = await AudioModule.requestRecordingPermissionsAsync();
        if (!permission.granted) {
          setChatError(MIC_DENIED);
          AccessibilityInfo.announceForAccessibility(MIC_DENIED);
          return;
        }
        await setAudioModeAsync(audioSession(true));
        await recorder.prepareToRecordAsync();
        recorder.record();
        if (!mountedRef.current) {
          if (recorder.isRecording) {
            await recorder.stop();
          }
          return;
        }
        setListening(true);
        setChatError(null);
        AccessibilityInfo.announceForAccessibility(
          "Listening. Tap again to send, or cancel.",
        );
      } catch (error) {
        if (recorderReleased(error)) {
          recorderRef.current = null;
        }
        if (!mountedRef.current) {
          return;
        }
        const message = readableError(error);
        setChatError(message);
        AccessibilityInfo.announceForAccessibility(message);
      } finally {
        voiceLock.current = false;
      }
    })();
  };

  const cancelListening = () => {
    if (voiceLock.current || sendingVoice || !listening) {
      return;
    }
    voiceLock.current = true;
    setListening(false);
    const recorder = recorderRef.current;
    void (async () => {
      try {
        if (recorder?.isRecording) {
          await recorder.stop();
        }
      } catch (error) {
        if (recorderReleased(error)) {
          recorderRef.current = null;
        }
      } finally {
        voiceLock.current = false;
      }
    })();
    AccessibilityInfo.announceForAccessibility("Recording cancelled.");
  };

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
              accessibilityRole="button"
              accessibilityLabel={voiceMuted ? "Unmute assistant voice" : "Mute assistant voice"}
              accessibilityHint={
                voiceMuted
                  ? "Assistant replies stay on screen."
                  : "Assistant replies are read aloud."
              }
              accessibilityState={{ selected: voiceMuted }}
              onPress={toggleVoice}
              hitSlop={4}
              style={({ pressed }) => [
                styles.chevronButton,
                pressed && styles.pressed,
              ]}
            >
              <Text
                style={styles.voiceEmoji}
                maxFontSizeMultiplier={1.8}
                importantForAccessibility="no"
                accessibilityElementsHidden
              >
                {voiceMuted ? "🔇" : "🔊"}
              </Text>
            </Pressable>
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
            contentContainerStyle={styles.transcriptContent}
            keyboardShouldPersistTaps="handled"
            keyboardDismissMode="interactive"
            accessibilityLabel="Conversation"
          >
            {messages.length === 0 ? (
              historyReady ? (
                <ChatBubble
                  message={{
                    id: "welcome",
                    role: "assistant",
                    text: welcomeLine(userNick),
                  }}
                />
              ) : null
            ) : (
              messages.map((message) => (
                <ChatBubble key={message.id} message={message} />
              ))
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
              <View style={styles.speakRow}>
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={
                    sendingVoice ? "Sending" : listening ? "Tap to stop" : "Tap to speak"
                  }
                  accessibilityHint={
                    listening
                      ? "Stops listening and sends what you said."
                      : "Starts listening. Tap again to send what you said."
                  }
                  accessibilityState={{
                    selected: listening,
                    busy: sendingVoice,
                    disabled: sendingVoice,
                  }}
                  disabled={sendingVoice}
                  hitSlop={space.md}
                  onPress={onSpeak}
                  style={[
                    styles.speak,
                    listening && styles.speakListening,
                    sendingVoice && styles.authButtonDisabled,
                  ]}
                >
                  <SoundWave active={listening} />
                  <Text
                    style={[styles.speakLabel, listening && styles.speakLabelListening]}
                    maxFontSizeMultiplier={1.8}
                    importantForAccessibility="no"
                    accessibilityElementsHidden
                  >
                    {sendingVoice ? "Sending" : listening ? "Tap to stop" : "Tap to speak"}
                  </Text>
                </Pressable>
                {listening ? (
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel="Cancel recording"
                    accessibilityHint="Discards what you said without sending a message."
                    hitSlop={space.md}
                    onPress={cancelListening}
                    style={styles.cancelSpeak}
                  >
                    <Text
                      style={styles.cancelSpeakLabel}
                      maxFontSizeMultiplier={1.8}
                      importantForAccessibility="no"
                      accessibilityElementsHidden
                    >
                      ✕
                    </Text>
                  </Pressable>
                ) : null}
              </View>
              <TextInput
                value={draft}
                onChangeText={setDraft}
                onSubmitEditing={sendDraft}
                placeholder="Message"
                placeholderTextColor={colors.inkMuted}
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

type MessagePart =
  | { kind: "text"; text: string }
  | { kind: "link"; label: string; href: string };

function welcomeLine(userNick: string): string {
  const nick = userNick.trim();
  const greeting = nick.length > 0 ? `Witam na Styrcie ${nick}!` : "Witam na Styrcie!";
  return `${greeting} Ja jestem Jadzia i chętnie pomogę z rozwiązaniem Twoich problemów lub znalezieniem spotkań towarzyskich w okolicy.`;
}

function ChatBubble({ message }: { message: ChatMessage }) {
  const assistant = message.role === "assistant";
  const parts =
    assistant && !message.pending ? linkParts(message.text) : [{ kind: "text" as const, text: message.text }];
  const links = parts.filter((part) => part.kind === "link");
  const [copied, setCopied] = useState(false);
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (copiedTimer.current != null) clearTimeout(copiedTimer.current);
    },
    [],
  );

  async function copyMessage() {
    let saved = false;
    try {
      saved = await Clipboard.setStringAsync(message.text);
    } catch {
      saved = false;
    }
    if (!saved) {
      AccessibilityInfo.announceForAccessibility("Could not copy the message.");
      return;
    }
    setCopied(true);
    AccessibilityInfo.announceForAccessibility("Copied");
    if (copiedTimer.current != null) clearTimeout(copiedTimer.current);
    copiedTimer.current = setTimeout(() => setCopied(false), 2000);
  }

  const spoken = message.pending
    ? "Assistant is thinking"
    : assistant
      ? `Assistant said: ${message.text}`
      : `You said: ${message.text}`;

  const sheetFill = assistant ? colors.white : colors.quaternary;

  return (
    <View
      accessibilityLiveRegion={message.pending ? "polite" : undefined}
      style={[styles.bubble, assistant && styles.assistantBubble, !message.pending && styles.bubbleWithCopy]}
    >
      <Text
        style={[
          styles.bubbleText,
          !message.pending && styles.bubbleTextBesideCopy,
          assistant && styles.assistantText,
          message.pending && styles.pendingText,
        ]}
        accessibilityRole="text"
        accessibilityLabel={spoken}
        accessibilityActions={links.map((link, index) => ({
          name: `open-link-${index}`,
          label: `Open ${link.label}`,
        }))}
        onAccessibilityAction={(event) => {
          const index = Number(event.nativeEvent.actionName.replace("open-link-", ""));
          const link = links[index];
          if (link) void openExternalLink(link.href);
        }}
      >
        {parts.map((part, index) =>
          part.kind === "text" ? (
            <Text key={`text-${index}`}>{part.text}</Text>
          ) : (
            <Text
              key={`link-${index}`}
              accessibilityRole="link"
              accessibilityLabel={part.label}
              accessibilityHint="Opens in your browser"
              style={styles.link}
              onPress={() => void openExternalLink(part.href)}
            >
              {part.label}
            </Text>
          ),
        )}
      </Text>
      {message.pending ? null : (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={copied ? "Message copied" : "Copy message"}
          accessibilityHint="Copies this message."
          onPress={() => void copyMessage()}
          hitSlop={space.sm}
          style={({ pressed }) => [styles.copyButton, pressed && styles.pressed]}
        >
          {({ pressed }) => (
            <CopyGlyph copied={copied} fill={pressed ? colors.line : sheetFill} />
          )}
        </Pressable>
      )}
    </View>
  );
}

function linkParts(source: string): MessagePart[] {
  const parts: MessagePart[] = [];
  const pattern = /\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)|https?:\/\/[^\s<>]+|www\.[^\s<>]+/gi;
  let cursor = 0;
  for (const match of source.matchAll(pattern)) {
    const index = match.index ?? 0;
    if (index > cursor) {
      parts.push({ kind: "text", text: source.slice(cursor, index) });
    }
    if (match[1] != null && match[2] != null) {
      const href = match[2];
      if (isHttpUrl(href)) {
        parts.push({ kind: "link", label: match[1], href });
      } else {
        parts.push({ kind: "text", text: match[0] });
      }
      cursor = index + match[0].length;
      continue;
    }
    const { href: trimmed, trailing } = splitTrailing(match[0]);
    const href = /^www\./i.test(trimmed) ? `https://${trimmed}` : trimmed;
    if (isHttpUrl(href)) {
      parts.push({ kind: "link", label: trimmed, href });
      if (trailing) parts.push({ kind: "text", text: trailing });
    } else {
      parts.push({ kind: "text", text: match[0] });
    }
    cursor = index + match[0].length;
  }
  if (cursor < source.length) {
    parts.push({ kind: "text", text: source.slice(cursor) });
  }
  return parts;
}

function splitTrailing(raw: string): { href: string; trailing: string } {
  let end = raw.length;
  while (end > 0) {
    const char = raw[end - 1] ?? "";
    if (char === ")") {
      const head = raw.slice(0, end);
      const opens = countChar(head, "(");
      const closes = countChar(head, ")");
      if (closes <= opens) break;
      end -= 1;
      continue;
    }
    if (".,;:!]\"'".includes(char)) {
      end -= 1;
      continue;
    }
    break;
  }
  return { href: raw.slice(0, end), trailing: raw.slice(end) };
}

function countChar(value: string, char: string): number {
  let count = 0;
  for (const item of value) {
    if (item === char) count += 1;
  }
  return count;
}

function isHttpUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return (url.protocol === "http:" || url.protocol === "https:") && url.hostname.length > 0;
  } catch {
    return false;
  }
}

async function openExternalLink(url: string) {
  if (!isHttpUrl(url)) return;
  try {
    await Linking.openURL(url);
  } catch {
    AccessibilityInfo.announceForAccessibility("Could not open the link.");
  }
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

function CopyGlyph({ copied, fill }: { copied: boolean; fill: string }) {
  return (
    <View
      pointerEvents="none"
      importantForAccessibility="no"
      accessibilityElementsHidden
      style={styles.copyIcon}
    >
      {copied ? (
        <View style={styles.copiedMark} />
      ) : (
        <>
          <View style={styles.copySheetBack} />
          <View style={[styles.copySheetFront, { backgroundColor: fill }]} />
        </>
      )}
    </View>
  );
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

function createRecorder(): AudioRecorder {
  const native = AudioModule as unknown as {
    AudioRecorder?: new (options: object) => AudioRecorder;
    AudioRecorderWeb?: new (options: object) => AudioRecorder;
  };
  const Factory = native.AudioRecorder ?? native.AudioRecorderWeb;
  if (!Factory) {
    throw new Error("Recording is not available.");
  }
  return new Factory(recordingOptions());
}

function recordingOptions() {
  const preset = RecordingPresets.HIGH_QUALITY;
  const common = {
    extension: preset.extension,
    sampleRate: preset.sampleRate,
    numberOfChannels: preset.numberOfChannels,
    bitRate: preset.bitRate,
    isMeteringEnabled: false,
  };
  if (Platform.OS === "ios") {
    return { ...common, ...preset.ios };
  }
  if (Platform.OS === "android") {
    return { ...common, ...preset.android };
  }
  return { ...common, ...preset.web };
}

function recorderReleased(error: unknown): boolean {
  return error instanceof Error && error.message.includes("already released");
}

function audioSession(allowsRecording: boolean) {
  return {
    playsInSilentMode: true,
    interruptionMode: allowsRecording ? ("doNotMix" as const) : ("duckOthers" as const),
    allowsRecording,
    shouldPlayInBackground: false,
    shouldRouteThroughEarpiece: false,
  };
}

function speechClip(uri: string): SpeechClip {
  if (Platform.OS === "web") {
    return { uri, name: "audio.webm", type: "audio/webm" };
  }
  return { uri, name: "audio.m4a", type: "audio/mp4" };
}

function SoundWave({ active }: { active: boolean }) {
  return (
    <View
      style={styles.wave}
      importantForAccessibility="no"
      accessibilityElementsHidden
    >
      {BAR_HEIGHTS.map((height, index) => (
        <View
          key={`bar-${index}`}
          style={[styles.bar, active && styles.barListening, { height }]}
        />
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
    backgroundColor: colors.tertiaryWash,
    borderTopLeftRadius: chatCornerRadius,
    borderTopRightRadius: chatCornerRadius,
    borderWidth: 0,
    paddingHorizontal: space.lg,
    paddingTop: CARD_PADDING_TOP,
    elevation: 8,
    shadowColor: "#000000",
    shadowOffset: { width: 0, height: -8 },
    shadowOpacity: 0.16,
    shadowRadius: 20,
  },
  header: {
    flexDirection: "row",
    justifyContent: "space-between",
    alignItems: "center",
  },
  voiceEmoji: {
    fontSize: 22,
    lineHeight: 28,
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
    borderColor: colors.ink,
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
    gap: space.sm,
    paddingBottom: space.md,
  },
  bubble: {
    alignSelf: "flex-end",
    maxWidth: "85%",
    backgroundColor: colors.quaternary,
    borderRadius: 16,
    paddingHorizontal: space.md,
    paddingVertical: space.md,
  },
  assistantBubble: {
    alignSelf: "flex-start",
    backgroundColor: colors.white,
    borderWidth: 1,
    borderColor: colors.line,
  },
  bubbleWithCopy: {
    flexDirection: "row",
    alignItems: "flex-start",
    paddingTop: space.xs,
    paddingEnd: space.xs,
  },
  bubbleText: {
    color: colors.ink,
    fontFamily: fonts.regular,
    fontSize: 16,
    lineHeight: 22,
  },
  bubbleTextBesideCopy: {
    flexShrink: 1,
    paddingTop: space.sm,
  },
  assistantText: {
    color: colors.ink,
  },
  pendingText: {
    color: colors.inkMuted,
    fontStyle: "italic",
  },
  link: {
    color: colors.ink,
    fontFamily: fonts.semibold,
    textDecorationLine: "underline",
  },
  copyButton: {
    width: 44,
    height: 44,
    alignItems: "center",
    justifyContent: "center",
    borderRadius: 22,
  },
  copyIcon: {
    width: 16,
    height: 16,
  },
  copySheetBack: {
    position: "absolute",
    top: 0,
    start: 0,
    width: 11,
    height: 11,
    borderWidth: 1.5,
    borderColor: colors.ink,
    borderRadius: 2,
  },
  copySheetFront: {
    position: "absolute",
    top: 4,
    start: 4,
    width: 11,
    height: 11,
    borderWidth: 1.5,
    borderColor: colors.ink,
    borderRadius: 2,
  },
  copiedMark: {
    width: 6,
    height: 11,
    marginTop: 1,
    marginStart: 5,
    borderBottomWidth: 2,
    borderRightWidth: 2,
    borderColor: colors.ink,
    transform: [{ rotate: "40deg" }],
  },
  pendingStatus: {
    color: colors.inkMuted,
    fontFamily: fonts.regular,
    fontSize: 16,
    fontStyle: "italic",
    lineHeight: 22,
    textAlign: "center",
    marginBottom: space.sm,
  },
  speakRow: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "center",
    gap: space.md,
    marginBottom: space.sm,
  },
  speak: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "center",
    gap: space.sm,
  },
  cancelSpeak: {
    width: 44,
    height: 44,
    alignItems: "center",
    justifyContent: "center",
  },
  cancelSpeakLabel: {
    color: "#D70015",
    fontFamily: fonts.semibold,
    fontSize: 22,
    lineHeight: 28,
  },
  speakListening: {},
  speakLabel: {
    color: colors.primary,
    fontFamily: fonts.semibold,
    fontSize: 15,
    lineHeight: 20,
  },
  speakLabelListening: {
    color: colors.primary,
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
    backgroundColor: colors.primary,
  },
  barListening: {
    backgroundColor: colors.primary,
  },
  input: {
    minHeight: 48,
    backgroundColor: colors.white,
    borderRadius: 999,
    borderWidth: 1,
    borderColor: colors.lineStrong,
    paddingHorizontal: space.lg,
    paddingVertical: space.md,
    color: colors.ink,
    fontFamily: fonts.regular,
    fontSize: 17,
    lineHeight: 22,
  },
  authActions: {
    gap: space.md,
  },
  fieldLabel: {
    color: colors.ink,
    fontFamily: fonts.semibold,
    fontSize: 15,
    lineHeight: 20,
  },
  authButton: {
    minHeight: 52,
    alignItems: "center",
    justifyContent: "center",
    borderRadius: 16,
    borderWidth: 1,
    borderColor: colors.lineStrong,
    backgroundColor: colors.white,
    paddingHorizontal: space.lg,
  },
  authButtonDisabled: {
    opacity: 0.4,
  },
  authLabel: {
    color: colors.ink,
    fontFamily: fonts.semibold,
    fontSize: 17,
    lineHeight: 22,
  },
  authSwitch: {
    minHeight: 44,
    alignItems: "center",
    justifyContent: "center",
  },
  authSwitchLabel: {
    color: colors.ink,
    fontFamily: fonts.regular,
    fontSize: 15,
    lineHeight: 20,
    textAlign: "center",
  },
  errorText: {
    color: colors.ink,
    fontFamily: fonts.regular,
    fontSize: 16,
    lineHeight: 22,
    textAlign: "center",
  },
  pressed: {
    backgroundColor: colors.line,
  },
});
