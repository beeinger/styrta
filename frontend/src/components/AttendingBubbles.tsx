import { useEffect, useRef, useState } from "react";
import {
  AccessibilityInfo,
  LayoutAnimation,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from "react-native";

import {
  describeAttendance,
  formatAttendance,
  formatEventStart,
  type MeetupEvent,
} from "../data/events";
import { isHostedEvent } from "../data/session";
import { colors, fonts, glassShadow, space } from "../theme";

type AttendingBubblesProps = {
  events: MeetupEvent[];
  top: number;
  bottom: number;
  start: number;
  end: number;
  expanded: boolean;
  concealed: boolean;
  onExpandedChange: (expanded: boolean) => void;
  onFocusEvent: (event: MeetupEvent) => void;
};

const unroll = {
  duration: 280,
  create: {
    type: LayoutAnimation.Types.easeInEaseOut,
    property: LayoutAnimation.Properties.scaleY,
  },
  update: {
    type: LayoutAnimation.Types.easeInEaseOut,
  },
  delete: {
    type: LayoutAnimation.Types.easeInEaseOut,
    property: LayoutAnimation.Properties.opacity,
  },
};

export function AttendingBubbles({
  events,
  top,
  bottom,
  start,
  end,
  expanded,
  concealed,
  onExpandedChange,
  onFocusEvent,
}: AttendingBubblesProps) {
  const [hiddenIds, setHiddenIds] = useState<string[]>([]);
  const [reduceMotion, setReduceMotion] = useState(false);
  const expandedRef = useRef(expanded);
  const visible = events.filter((event) => !hiddenIds.includes(event.id));

  if (expandedRef.current !== expanded) {
    if (!reduceMotion) {
      LayoutAnimation.configureNext(unroll);
    }
    expandedRef.current = expanded;
  }

  useEffect(() => {
    let active = true;
    void AccessibilityInfo.isReduceMotionEnabled().then((enabled) => {
      if (active) {
        setReduceMotion(enabled);
      }
    });
    const subscription = AccessibilityInfo.addEventListener(
      "reduceMotionChanged",
      (enabled) => {
        setReduceMotion(enabled);
      },
    );
    return () => {
      active = false;
      subscription.remove();
    };
  }, []);

  useEffect(() => {
    if (visible.length === 0) {
      onExpandedChange(false);
    }
  }, [onExpandedChange, visible.length]);

  if (concealed || visible.length === 0) {
    return null;
  }

  const hide = (event: MeetupEvent) => {
    setHiddenIds((current) =>
      current.includes(event.id) ? current : [...current, event.id],
    );
    AccessibilityInfo.announceForAccessibility(`${event.title} hidden`);
  };

  return (
    <View
      pointerEvents="box-none"
      style={[
        styles.anchor,
        { top, start, end },
        expanded ? { bottom } : null,
      ]}
    >
      {expanded ? (
        <View
          accessibilityViewIsModal
          pointerEvents="box-none"
          style={styles.expanded}
        >
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Close your events"
            onPress={() => {
              onExpandedChange(false);
              AccessibilityInfo.announceForAccessibility("Events closed");
            }}
            hitSlop={space.xs}
            style={({ pressed }) => [
              styles.bubble,
              styles.closeBubble,
              pressed && styles.bubblePressed,
            ]}
          >
            <Text
              style={styles.closeGlyph}
              maxFontSizeMultiplier={1.6}
              importantForAccessibility="no"
              accessibilityElementsHidden
            >
              ×
            </Text>
          </Pressable>
          <ScrollView
            style={styles.list}
            contentContainerStyle={styles.listContent}
            keyboardShouldPersistTaps="handled"
          >
            {visible.map((event) => (
              <EventRow
                key={event.id}
                event={event}
                onFocus={() => {
                  onExpandedChange(false);
                  onFocusEvent(event);
                }}
                onHide={() => hide(event)}
              />
            ))}
          </ScrollView>
        </View>
      ) : (
        <View style={styles.stack}>
          {visible.map((event, index) => (
            <Pressable
              key={event.id}
              accessibilityRole="button"
              accessibilityLabel={`${event.title}, ${formatEventStart(event.startsAt)}`}
              accessibilityHint="Opens the list of events you are attending"
              onPress={() => {
                onExpandedChange(true);
                AccessibilityInfo.announceForAccessibility(
                  "Showing events you are attending",
                );
              }}
              hitSlop={space.xs}
              style={({ pressed }) => [
                styles.bubble,
                index > 0 && styles.bubblePeek,
                {
                  zIndex: visible.length - index,
                  elevation: glassShadow.elevation + visible.length - index,
                },
                pressed && styles.bubblePressed,
              ]}
            >
              <Text
                style={styles.emoji}
                maxFontSizeMultiplier={1.6}
                importantForAccessibility="no"
                accessibilityElementsHidden
              >
                {event.emoji}
              </Text>
            </Pressable>
          ))}
        </View>
      )}
    </View>
  );
}

type EventRowProps = {
  event: MeetupEvent;
  onFocus: () => void;
  onHide: () => void;
};

function EventRow({ event, onFocus, onHide }: EventRowProps) {
  const when = formatEventStart(event.startsAt);
  const attendance = describeAttendance(event.signedCount, event.capacity);
  const hosting = event.hostedByMe === true || isHostedEvent(event.id);
  const role = hosting ? "You are hosting" : "You are attending";
  return (
    <View style={styles.row}>
      <View
        style={[
          styles.emojiCircle,
          { backgroundColor: hosting ? colors.primary : colors.secondary },
        ]}
      >
        <Text
          style={styles.emoji}
          maxFontSizeMultiplier={1.6}
          importantForAccessibility="no"
          accessibilityElementsHidden
        >
          {event.emoji}
        </Text>
      </View>
      <View style={styles.pill}>
        <View
          accessible
          accessibilityLabel={`${event.hostName}, ${event.title}, ${when}, ${attendance}. ${role}`}
          style={styles.details}
        >
          <Text style={styles.host}>{event.hostName}</Text>
          <Text style={styles.title}>{event.title}</Text>
          <Text style={styles.time}>{when}</Text>
          <Text style={styles.count}>
            {formatAttendance(event.signedCount, event.capacity)}
          </Text>
        </View>
        <View style={styles.actions}>
          <RowButton
            label={`Show ${event.title} on the map`}
            glyph="📍"
            onPress={onFocus}
          />
          <RowButton label={`Hide ${event.title}`} glyph="✓" onPress={onHide} />
          <RowButton label={`Remove ${event.title}`} glyph="✕" onPress={onHide} />
        </View>
      </View>
    </View>
  );
}

type RowButtonProps = {
  label: string;
  glyph: string;
  onPress: () => void;
};

function RowButton({ label, glyph, onPress }: RowButtonProps) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={onPress}
      style={styles.rowButton}
    >
      {({ pressed }) => (
        <>
          {pressed ? <View style={styles.rowButtonPressed} /> : null}
          <Text
            style={styles.rowGlyph}
            maxFontSizeMultiplier={1.4}
            importantForAccessibility="no"
            accessibilityElementsHidden
          >
            {glyph}
          </Text>
        </>
      )}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  anchor: {
    position: "absolute",
  },
  stack: {
    alignSelf: "flex-start",
    marginTop: "10%",
  },
  expanded: {
    flex: 1,
    minHeight: 0,
    paddingTop: "10%",
    gap: space.sm,
    justifyContent: "flex-start",
  },
  bubble: {
    width: 48,
    height: 48,
    borderRadius: 24,
    backgroundColor: colors.glass,
    alignItems: "center",
    justifyContent: "center",
    ...glassShadow,
  },
  closeBubble: {
    alignSelf: "flex-start",
  },
  bubblePeek: {
    marginTop: -40,
  },
  bubblePressed: {
    backgroundColor: colors.glassSelected,
  },
  emoji: {
    fontSize: 24,
    lineHeight: 30,
    textAlign: "center",
  },
  closeGlyph: {
    color: colors.ink,
    fontFamily: fonts.semibold,
    fontSize: 28,
    lineHeight: 32,
    textAlign: "center",
  },
  list: {
    flexGrow: 0,
    flexShrink: 1,
  },
  listContent: {
    gap: space.sm,
    paddingVertical: space.xs,
  },
  row: {
    flexDirection: "row",
    alignItems: "center",
    gap: space.sm,
  },
  emojiCircle: {
    width: 48,
    height: 48,
    borderRadius: 24,
    alignItems: "center",
    justifyContent: "center",
    ...glassShadow,
  },
  pill: {
    flex: 1,
    minWidth: 0,
    flexDirection: "row",
    alignItems: "center",
    gap: space.sm,
    backgroundColor: colors.glass,
    borderRadius: 999,
    paddingVertical: space.md,
    paddingHorizontal: space.lg,
    ...glassShadow,
  },
  details: {
    flex: 1,
    minWidth: 0,
    gap: space.xs,
  },
  host: {
    color: colors.inkMuted,
    fontFamily: fonts.regular,
    fontSize: 13,
    lineHeight: 18,
  },
  title: {
    color: colors.ink,
    fontFamily: fonts.semibold,
    fontSize: 16,
    lineHeight: 22,
  },
  time: {
    color: colors.inkMuted,
    fontFamily: fonts.regular,
    fontSize: 13,
    lineHeight: 18,
  },
  count: {
    color: colors.ink,
    fontFamily: fonts.semibold,
    fontSize: 15,
    lineHeight: 20,
  },
  actions: {
    flexDirection: "row",
    alignItems: "center",
    flexShrink: 0,
    gap: space.xs,
  },
  rowButton: {
    width: 44,
    height: 44,
    borderRadius: 22,
    backgroundColor: colors.quaternaryWash,
    alignItems: "center",
    justifyContent: "center",
    overflow: "hidden",
  },
  rowButtonPressed: {
    ...StyleSheet.absoluteFill,
    backgroundColor: colors.quaternary,
    opacity: 0.24,
  },
  rowGlyph: {
    fontSize: 20,
    lineHeight: 26,
    textAlign: "center",
  },
});
