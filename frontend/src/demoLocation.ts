type Coordinates = {
  latitude: number;
  longitude: number;
};

function enabled(value: string | undefined): boolean {
  return value?.trim().toLowerCase() === "true";
}

function coordinate(value: string | undefined, min: number, max: number): number | null {
  if (value == null || value.trim() === "") {
    return null;
  }
  const number = Number(value);
  if (!Number.isFinite(number) || number < min || number > max) {
    return null;
  }
  return number;
}

const latitude = coordinate(process.env.EXPO_PUBLIC_DEMO_LATITUDE, -90, 90);
const longitude = coordinate(process.env.EXPO_PUBLIC_DEMO_LONGITUDE, -180, 180);

/** Fixed stand-in for the device location, used when the demo flag is on. */
export const demoLocation: Coordinates | null =
  enabled(process.env.EXPO_PUBLIC_DEMO_LOCATION) && latitude != null && longitude != null
    ? { latitude, longitude }
    : null;
