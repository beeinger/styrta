import { Pressable, StyleSheet, View, I18nManager } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { Icon, type IconName } from '../icons';
import { colors, glassShadow, space } from '../theme';

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
          name="plus"
          disabled={!canZoomIn}
          onPress={onZoomIn}
        />
        <ZoomButton
          label="Zoom out"
          hint="Shows a wider area of the map"
          name="minus"
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
          name="locate"
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
  name: IconName;
  disabled: boolean;
  onPress: () => void;
};

function ZoomButton({ label, hint, name, disabled, onPress }: ZoomButtonProps) {
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
      <Icon name={name} size={20} color={disabled ? colors.inkMuted : colors.ink} />
    </Pressable>
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
    backgroundColor: colors.glassSelected,
  },
});
