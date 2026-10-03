import { useEffect, useRef, useState } from "react";
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

type AiChatSheetProps = {
  onHeightChange: (height: number) => void;
  onTopChange: (offsetFromBottom: number) => void;
};

type ChatMessage = {
  id: string;
  text: string;
};

const BAR_HEIGHTS = [7, 13, 19, 13, 7];
const CARD_PADDING_TOP = 4;

export function AiChatSheet({ onHeightChange, onTopChange }: AiChatSheetProps) {
  const insets = useSafeAreaInsets();
  const { height: windowHeight } = useWindowDimensions();
  const transcriptRef = useRef<ScrollView>(null);
  const nextMessageId = useRef(0);
  const headerHeight = useRef(0);
  const composerHeight = useRef(0);
  const cardHeight = useRef(0);
  const [expanded, setExpanded] = useState(true);
  const [draft, setDraft] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [keyboardInset, setKeyboardInset] = useState(0);
  const [reduceMotion, setReduceMotion] = useState(false);

  const halfScreen = Math.round(windowHeight * 0.5);
  const panelHeight = Math.min(halfScreen, windowHeight - keyboardInset);

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
    if (Platform.OS !== "ios") {
      return;
    }
    const show = Keyboard.addListener("keyboardWillShow", (event) => {
      setKeyboardInset(event.endCoordinates.height);
    });
    const hide = Keyboard.addListener("keyboardWillHide", () => {
      setKeyboardInset(0);
    });
    return () => {
      show.remove();
      hide.remove();
    };
  }, []);

  useEffect(() => {
    if (!expanded) {
      return;
    }
    const frame = requestAnimationFrame(() => {
      transcriptRef.current?.scrollToEnd({ animated: !reduceMotion });
    });
    return () => cancelAnimationFrame(frame);
  }, [expanded, messages, reduceMotion]);

  const toggleExpanded = () => {
    const next = !expanded;
    setExpanded(next);
    AccessibilityInfo.announceForAccessibility(
      next ? "Chat expanded" : "Chat minimized. Conversation hidden.",
    );
  };

  const sendDraft = () => {
    const text = draft.trim();
    if (!text) {
      return;
    }
    nextMessageId.current += 1;
    const message = { id: `message-${nextMessageId.current}`, text };
    setMessages((current) => [...current, message]);
    setDraft("");
    AccessibilityInfo.announceForAccessibility("Message sent");
  };

  const bottomInset = keyboardInset > 0 ? 12 : Math.max(insets.bottom, 12);

  const publishMapClearance = (header: number, composer: number) => {
    if (header === 0 || composer === 0) {
      return;
    }
    onHeightChange(header + composer + bottomInset + CARD_PADDING_TOP);
  };

  const publishTop = (height: number) => {
    cardHeight.current = height;
    onTopChange(height + keyboardInset);
  };

  useEffect(() => {
    publishMapClearance(headerHeight.current, composerHeight.current);
  }, [bottomInset, onHeightChange]);

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
          expanded ? { height: panelHeight } : null,
          { paddingBottom: bottomInset },
        ]}
      >
        <View
          style={styles.header}
          onLayout={(event) => {
            headerHeight.current = event.nativeEvent.layout.height;
            publishMapClearance(headerHeight.current, composerHeight.current);
          }}
        >
          <Pressable
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
        {expanded ? (
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
              messages.map((message) => (
                <View
                  key={message.id}
                  accessible
                  accessibilityRole="text"
                  accessibilityLabel={`You said: ${message.text}`}
                  style={styles.bubble}
                >
                  <Text
                    style={styles.bubbleText}
                    importantForAccessibility="no"
                    accessibilityElementsHidden
                  >
                    {message.text}
                  </Text>
                </View>
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
        </View>
      </View>
    </View>
  );
}

function ChevronGlyph({ expanded }: { expanded: boolean }) {
  return (
    <View
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
  bubbleText: {
    color: "#FFFFFF",
    fontSize: 16,
    lineHeight: 22,
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
  pressed: {
    backgroundColor: "#E5E5EA",
  },
});
