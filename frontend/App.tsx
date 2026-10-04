import { Nunito_400Regular, Nunito_600SemiBold } from '@expo-google-fonts/nunito';
import { useFonts } from 'expo-font';
import { StatusBar } from 'expo-status-bar';
import { SafeAreaProvider } from 'react-native-safe-area-context';

import { MapScreen } from './src/components/MapScreen';
import { fonts } from './src/theme';

export default function App() {
  const [fontsLoaded, fontError] = useFonts({
    [fonts.regular]: Nunito_400Regular,
    [fonts.semibold]: Nunito_600SemiBold,
  });

  if (!fontsLoaded && !fontError) {
    return null;
  }

  return (
    <SafeAreaProvider>
      <StatusBar style="dark" />
      <MapScreen />
    </SafeAreaProvider>
  );
}
