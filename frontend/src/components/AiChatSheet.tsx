import { StyleSheet, Text, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

const LABEL =
  'AI chat. Coming soon. You will be able to talk with the assistant here.';

type AiChatSheetProps = {
  onHeightChange: (height: number) => void;
};

export function AiChatSheet({ onHeightChange }: AiChatSheetProps) {
  const insets = useSafeAreaInsets();

  return (
    <View
      pointerEvents="box-none"
      onLayout={(event) => {
        onHeightChange(event.nativeEvent.layout.height);
      }}
      style={[
        styles.dock,
        {
          paddingBottom: insets.bottom + 12,
          paddingStart: Math.max(16, insets.left),
          paddingEnd: Math.max(16, insets.right),
        },
      ]}
    >
      <View
        accessible
        accessibilityRole="text"
        accessibilityLabel={LABEL}
        style={styles.card}
      >
        <Text
          style={styles.title}
          importantForAccessibility="no"
          accessibilityElementsHidden
        >
          AI chat
        </Text>
        <Text
          style={styles.body}
          importantForAccessibility="no"
          accessibilityElementsHidden
        >
          Coming soon. You will be able to talk with the assistant here.
        </Text>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  dock: {
    position: 'absolute',
    start: 0,
    end: 0,
    bottom: 0,
  },
  card: {
    backgroundColor: '#FFFFFF',
    borderColor: '#3A3A3C',
    borderRadius: 24,
    borderWidth: 1,
    paddingHorizontal: 20,
    paddingVertical: 16,
    elevation: 6,
    shadowColor: '#000000',
    shadowOffset: { width: 0, height: 4 },
    shadowOpacity: 0.18,
    shadowRadius: 12,
  },
  title: {
    color: '#1C1C1E',
    fontSize: 20,
    fontWeight: '700',
    lineHeight: 26,
  },
  body: {
    color: '#3A3A3C',
    fontSize: 16,
    lineHeight: 22,
    marginTop: 4,
  },
});
