import { I18nManager, Pressable, StyleSheet, Text, View } from 'react-native';
import type { ReactNode } from 'react';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

type MapZoomControlsProps = {
  bottom: number;
  canZoomIn: boolean;
  canZoomOut: boolean;
  canCenter: boolean;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onCenter: () => void;
};

export function MapZoomControls({
  bottom,
  canZoomIn,
  canZoomOut,
  canCenter,
  onZoomIn,
  onZoomOut,
  onCenter,
}: MapZoomControlsProps) {
  const insets = useSafeAreaInsets();
  const endInset = I18nManager.isRTL ? insets.left : insets.right;
  const startInset = I18nManager.isRTL ? insets.right : insets.left;

  return (
    <>
      <View
        accessible={false}
        pointerEvents="box-none"
        style={[styles.group, { bottom, end: Math.max(16, endInset) }]}
      >
        <ZoomButton
          label="Zoom in"
          hint="Shows a smaller area of the map"
          glyph="+"
          disabled={!canZoomIn}
          onPress={onZoomIn}
        />
        <ZoomButton
          label="Zoom out"
          hint="Shows a wider area of the map"
          glyph="−"
          disabled={!canZoomOut}
          onPress={onZoomOut}
        />
      </View>
      <View
        accessible={false}
        pointerEvents="box-none"
        style={[styles.group, { bottom, start: Math.max(16, startInset) }]}
      >
        <ZoomButton
          label="Center"
          hint="Centers the map on your location"
          glyph={<CenterGlyph />}
          disabled={!canCenter}
          onPress={onCenter}
        />
      </View>
    </>
  );
}

type ZoomButtonProps = {
  label: string;
  hint: string;
  glyph: ReactNode;
  disabled: boolean;
  onPress: () => void;
};

function ZoomButton({ label, hint, glyph, disabled, onPress }: ZoomButtonProps) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityHint={hint}
      accessibilityState={{ disabled }}
      disabled={disabled}
      onPress={onPress}
      hitSlop={4}
      android_ripple={{ color: 'rgba(28, 28, 30, 0.12)' }}
      style={({ pressed }) => [
        styles.button,
        disabled && styles.buttonDisabled,
        pressed && !disabled && styles.buttonPressed,
      ]}
    >
      {typeof glyph === 'string' ? (
        <Text
          style={styles.glyph}
          maxFontSizeMultiplier={1.6}
          importantForAccessibility="no"
          accessibilityElementsHidden
        >
          {glyph}
        </Text>
      ) : (
        glyph
      )}
    </Pressable>
  );
}

function CenterGlyph() {
  return (
    <View
      style={styles.target}
      importantForAccessibility="no"
      accessibilityElementsHidden
    >
      <View style={styles.targetRing} />
      <View style={styles.targetDot} />
    </View>
  );
}

const styles = StyleSheet.create({
  group: {
    position: 'absolute',
    gap: 8,
  },
  button: {
    width: 48,
    height: 48,
    borderRadius: 24,
    borderWidth: 2,
    borderColor: '#1C1C1E',
    backgroundColor: '#FFFFFF',
    alignItems: 'center',
    justifyContent: 'center',
    elevation: 4,
    shadowColor: '#000000',
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.2,
    shadowRadius: 4,
  },
  buttonDisabled: {
    opacity: 0.4,
  },
  buttonPressed: {
    backgroundColor: '#E5E5EA',
  },
  glyph: {
    color: '#1C1C1E',
    fontSize: 28,
    lineHeight: 32,
    fontWeight: '500',
    textAlign: 'center',
  },
  target: {
    width: 22,
    height: 22,
    alignItems: 'center',
    justifyContent: 'center',
  },
  targetRing: {
    position: 'absolute',
    width: 18,
    height: 18,
    borderRadius: 9,
    borderWidth: 2,
    borderColor: '#1C1C1E',
  },
  targetDot: {
    width: 6,
    height: 6,
    borderRadius: 3,
    backgroundColor: '#1C1C1E',
  },
});
