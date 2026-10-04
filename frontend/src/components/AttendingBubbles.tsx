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
import { Icon, type IconName } from "../icons";
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
            <Icon name="close" size={20} color={colors.ink} />
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
  const place = event.placeName?.trim() ?? "";
  const hosting = event.hostedByMe === true || isHostedEvent(event.id);
  const role = hosting ? "You are hosting" : "You are attending";
  const summary = [event.title, when, place, attendance].filter(Boolean).join(", ");
  return (
    <View style={styles.card}>
      <View
        accessible
        accessibilityLabel={`${summary}. ${role}`}
        style={styles.cardMain}
      >
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
        <View style={styles.details}>
          <Text style={styles.title} numberOfLines={2}>
            {event.title}
          </Text>
          <Text style={styles.meta} numberOfLines={1}>
            {when}
          </Text>
          {place ? (
            <Text style={styles.meta} numberOfLines={2}>
              {place}
            </Text>
          ) : null}
          <View style={styles.people}>
            <Icon name="users" size={15} color={colors.ink} />
            <Text style={styles.count}>
              {formatAttendance(event.signedCount, event.capacity)}
            </Text>
          </View>
        </View>
      </View>
      <View style={styles.actions}>
        <RowButton
          label={`Show ${event.title} on the map`}
          name="pin"
          onPress={onFocus}
        />
        <RowButton label={`Hide ${event.title}`} name="check" onPress={onHide} />
        <RowButton label={`Remove ${event.title}`} name="close" onPress={onHide} />
      </View>
    </View>
  );
}

type RowButtonProps = {
  label: string;
  name: IconName;
  onPress: () => void;
};

function RowButton({ label, name, onPress }: RowButtonProps) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={onPress}
      style={({ pressed }) => [styles.rowButton, pressed && styles.rowButtonPressed]}
    >
      <Icon name={name} size={18} color={colors.ink} />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  anchor: {
    position: "absolute",
  },
  stack: {
    alignSelf: "flex-start",
    marginTop: space.xl,
  },
  expanded: {
    flex: 1,
    minHeight: 0,
    paddingTop: space.lg,
    gap: space.md,
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
    fontSize: 22,
    lineHeight: 28,
    textAlign: "center",
  },
  list: {
    alignSelf: "stretch",
    flexGrow: 0,
    flexShrink: 1,
  },
  listContent: {
    gap: space.sm,
    paddingBottom: space.sm,
    width: "100%",
  },
  card: {
    alignSelf: "stretch",
    backgroundColor: colors.white,
    borderRadius: 20,
    padding: space.md,
    gap: space.md,
    ...glassShadow,
  },
  cardMain: {
    flexDirection: "row",
    alignItems: "flex-start",
    gap: space.md,
  },
  emojiCircle: {
    width: 44,
    height: 44,
    borderRadius: 22,
    alignItems: "center",
    justifyContent: "center",
  },
  details: {
    flex: 1,
    minWidth: 0,
    gap: 2,
  },
  title: {
    color: colors.ink,
    fontFamily: fonts.semibold,
    fontSize: 16,
    lineHeight: 21,
  },
  meta: {
    color: colors.inkMuted,
    fontFamily: fonts.regular,
    fontSize: 13,
    lineHeight: 18,
  },
  people: {
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
    marginTop: 4,
  },
  count: {
    color: colors.ink,
    fontFamily: fonts.semibold,
    fontSize: 14,
    lineHeight: 18,
  },
  actions: {
    flexDirection: "row",
    justifyContent: "flex-end",
    alignItems: "center",
    gap: space.sm,
  },
  rowButton: {
    width: 40,
    height: 40,
    borderRadius: 20,
    backgroundColor: colors.canvas,
    alignItems: "center",
    justifyContent: "center",
  },
  rowButtonPressed: {
    backgroundColor: colors.secondary,
  },
});
