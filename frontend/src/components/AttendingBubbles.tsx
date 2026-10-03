import { useEffect, useState } from "react";
import {
  AccessibilityInfo,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from "react-native";

import { formatEventStart, type MeetupEvent } from "../data/events";

type AttendingBubblesProps = {
  events: MeetupEvent[];
  top: number;
  start: number;
  end: number;
  expanded: boolean;
  onExpandedChange: (expanded: boolean) => void;
  onFocusEvent: (event: MeetupEvent) => void;
};

export function AttendingBubbles({
  events,
  top,
  start,
  end,
  expanded,
  onExpandedChange,
  onFocusEvent,
}: AttendingBubblesProps) {
  const [hiddenIds, setHiddenIds] = useState<string[]>([]);
  const visible = events.filter((event) => !hiddenIds.includes(event.id));

  useEffect(() => {
    if (visible.length === 0) {
      onExpandedChange(false);
    }
  }, [onExpandedChange, visible.length]);

  if (visible.length === 0) {
    return null;
  }

  const hide = (event: MeetupEvent) => {
    setHiddenIds((current) =>
      current.includes(event.id) ? current : [...current, event.id],
    );
    AccessibilityInfo.announceForAccessibility(`${event.title} hidden`);
  };

  return (
    <View pointerEvents="box-none" style={[styles.anchor, { top, start, end }]}>
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
            hitSlop={4}
            style={({ pressed }) => [
              styles.bubble,
              styles.closeBubble,
              pressed && styles.pressed,
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
          <View style={styles.panel}>
            <ScrollView
              style={styles.list}
              contentContainerStyle={styles.listContent}
              keyboardShouldPersistTaps="handled"
            >
              {visible.map((event, index) => (
                <EventRow
                  key={event.id}
                  event={event}
                  last={index === visible.length - 1}
                  onFocus={() => {
                    onExpandedChange(false);
                    onFocusEvent(event);
                  }}
                  onHide={() => hide(event)}
                />
              ))}
            </ScrollView>
          </View>
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
              hitSlop={4}
              style={({ pressed }) => [
                styles.bubble,
                index > 0 && styles.bubblePeek,
                {
                  zIndex: visible.length - index,
                  elevation: visible.length - index,
                },
                pressed && styles.pressed,
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
  last: boolean;
  onFocus: () => void;
  onHide: () => void;
};

function EventRow({ event, last, onFocus, onHide }: EventRowProps) {
  const when = formatEventStart(event.startsAt);
  return (
    <View style={[styles.row, !last && styles.rowDivider]}>
      <View
        accessible
        accessibilityLabel={`${event.hostName}, ${event.title}, ${when}, ${event.signedCount} of ${event.capacity} people`}
        style={styles.details}
      >
        <Text style={styles.host}>{event.hostName}</Text>
        <Text style={styles.title}>{event.title}</Text>
        <Text style={styles.time}>{when}</Text>
        <Text style={styles.count}>
          {event.signedCount}/{event.capacity}
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
      hitSlop={4}
      style={({ pressed }) => [styles.rowButton, pressed && styles.pressed]}
    >
      <Text
        style={styles.rowGlyph}
        maxFontSizeMultiplier={1.4}
        importantForAccessibility="no"
        accessibilityElementsHidden
      >
        {glyph}
      </Text>
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
    marginTop: "10%",
    gap: 8,
  },
  bubble: {
    width: 48,
    height: 48,
    borderRadius: 24,
    borderWidth: 2,
    borderColor: "#1C1C1E",
    backgroundColor: "#FFFFFF",
    alignItems: "center",
    justifyContent: "center",
    elevation: 4,
    shadowColor: "#000000",
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.2,
    shadowRadius: 4,
  },
  closeBubble: {
    alignSelf: "flex-start",
  },
  bubblePeek: {
    marginTop: -40,
  },
  pressed: {
    backgroundColor: "#E5E5EA",
  },
  emoji: {
    fontSize: 24,
    lineHeight: 30,
    textAlign: "center",
  },
  closeGlyph: {
    color: "#1C1C1E",
    fontSize: 28,
    lineHeight: 32,
    fontWeight: "500",
    textAlign: "center",
  },
  panel: {
    alignSelf: "stretch",
    maxHeight: 320,
    backgroundColor: "#FFFFFF",
    borderColor: "#1C1C1E",
    borderRadius: 16,
    borderWidth: 2,
    overflow: "hidden",
    elevation: 4,
    shadowColor: "#000000",
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.2,
    shadowRadius: 4,
  },
  list: {
    flexGrow: 0,
  },
  listContent: {
    paddingVertical: 4,
  },
  row: {
    flexDirection: "row",
    alignItems: "center",
    gap: 8,
    paddingStart: 14,
    paddingEnd: 8,
    paddingVertical: 10,
  },
  rowDivider: {
    borderBottomColor: "#E5E5EA",
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  details: {
    flex: 1,
  },
  host: {
    color: "#636366",
    fontSize: 13,
    lineHeight: 18,
  },
  title: {
    marginTop: 2,
    color: "#1C1C1E",
    fontSize: 16,
    fontWeight: "600",
    lineHeight: 22,
  },
  time: {
    marginTop: 2,
    color: "#636366",
    fontSize: 13,
    lineHeight: 18,
  },
  count: {
    marginTop: 4,
    color: "#1C1C1E",
    fontSize: 15,
    fontWeight: "600",
    lineHeight: 20,
  },
  actions: {
    flexDirection: "row",
    alignItems: "center",
    gap: 4,
  },
  rowButton: {
    width: 44,
    height: 44,
    borderRadius: 22,
    alignItems: "center",
    justifyContent: "center",
  },
  rowGlyph: {
    fontSize: 20,
    lineHeight: 26,
    textAlign: "center",
  },
});
