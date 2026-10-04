import { I18nManager, Pressable, StyleSheet, Text, View } from 'react-native';
import type { ReactNode } from 'react';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { colors, fonts, glassShadow, space } from '../theme';

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
      style={styles.targetRing}
      importantForAccessibility="no"
      accessibilityElementsHidden
    >
      <View style={styles.targetDot} />
    </View>
  );
}

const styles = StyleSheet.create({
  group: {
    position: 'absolute',
    gap: space.sm,
  },
  button: {
    width: 48,
    height: 48,
    borderRadius: 24,
    backgroundColor: colors.glass,
    alignItems: 'center',
    justifyContent: 'center',
    ...glassShadow,
  },
  buttonDisabled: {
    opacity: 0.4,
  },
  buttonPressed: {
    backgroundColor: colors.white,
  },
  glyph: {
    color: colors.ink,
    fontSize: 28,
    lineHeight: 28,
    height: 28,
    width: 28,
    fontFamily: fonts.semibold,
    textAlign: 'center',
    textAlignVertical: 'center',
    includeFontPadding: false,
  },
  targetRing: {
    width: 18,
    height: 18,
    borderRadius: 9,
    borderWidth: 2,
    borderColor: colors.ink,
    alignItems: 'center',
    justifyContent: 'center',
  },
  targetDot: {
    width: 6,
    height: 6,
    borderRadius: 3,
    backgroundColor: colors.ink,
  },
});
