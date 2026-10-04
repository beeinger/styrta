import { Nunito_400Regular, Nunito_600SemiBold } from '@expo-google-fonts/nunito';
import { useFonts } from 'expo-font';
import { StatusBar } from 'expo-status-bar';
import type { ReactNode } from 'react';
import { Platform, StyleSheet, useWindowDimensions, View } from 'react-native';
import { SafeAreaProvider } from 'react-native-safe-area-context';

import { MapScreen } from './src/components/MapScreen';
import { colors, fonts } from './src/theme';

const PHONE_WIDTH = 390;
const PHONE_ASPECT = 19.5 / 9;

function phoneFrame(windowWidth: number, windowHeight: number) {
  const width = Math.min(windowWidth, PHONE_WIDTH);
  const height = Math.min(windowHeight, width * PHONE_ASPECT);
  if (windowWidth - width < 1 && windowHeight - height < 1) {
    return { width: windowWidth, height: windowHeight, framed: false };
  }
  return { width, height, framed: true };
}

function WebShell({ children }: { children: ReactNode }) {
  const viewport = useWindowDimensions();
  const frame = phoneFrame(viewport.width, viewport.height);
  return (
    <View style={styles.stage}>
      <View
        style={[
          styles.phone,
          { width: frame.width, height: frame.height },
          frame.framed && styles.phoneFramed,
        ]}
      >
        {children}
      </View>
    </View>
  );
}

export default function App() {
  const [fontsLoaded, fontError] = useFonts({
    [fonts.regular]: Nunito_400Regular,
    [fonts.semibold]: Nunito_600SemiBold,
  });

  if (!fontsLoaded && !fontError) {
    return null;
  }

  const screen = (
    <SafeAreaProvider style={styles.screen}>
      <StatusBar style="dark" />
      <MapScreen />
    </SafeAreaProvider>
  );

  if (Platform.OS !== 'web') {
    return screen;
  }
  return <WebShell>{screen}</WebShell>;
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
  },
  stage: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: colors.ink,
  },
  phone: {
    overflow: 'hidden',
    backgroundColor: colors.white,
  },
  phoneFramed: {
    borderRadius: 28,
  },
});
