import type { ConfigContext, ExpoConfig } from 'expo/config';

export default ({ config }: ConfigContext): ExpoConfig => {
  return {
    ...config,
    name: config.name ?? 'styrta',
    slug: config.slug ?? 'styrta',
    ios: {
      ...config.ios,
      bundleIdentifier: 'com.styrta.app',
    },
    android: {
      ...config.android,
      package: 'com.styrta.app',
    },
    plugins: [
      ...(config.plugins ?? []),
      [
        'expo-location',
        {
          locationWhenInUsePermission:
            'Allow Styrta to use your location to center the map on you.',
        },
      ],
      [
        'expo-audio',
        {
          microphonePermission: 'Allow Styrta to use the microphone so you can speak.',
          enableBackgroundRecording: false,
          enableBackgroundPlayback: false,
        },
      ],
    ],
  };
};
