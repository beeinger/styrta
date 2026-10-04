import eventsData from "./events.json";

export type MeetupEvent = {
  id: string;
  emoji: string;
  title: string;
  signedCount: number;
  capacity: number | null;
  latitude: number;
  longitude: number;
  hostName: string;
  startsAt: string;
};

export function formatAttendance(signedCount: number, capacity: number | null): string {
  if (capacity == null) {
    return String(signedCount);
  }
  return `${signedCount}/${capacity}`;
}

export function describeAttendance(signedCount: number, capacity: number | null): string {
  if (capacity == null) {
    return `${signedCount} people`;
  }
  return `${signedCount} of ${capacity} people`;
}

export function formatEventStart(startsAt: string): string {
  return new Intl.DateTimeFormat("en-GB", {
    weekday: "short",
    day: "numeric",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
    timeZone: "Europe/Warsaw",
  }).format(new Date(startsAt));
}

export const events = eventsData as MeetupEvent[];

export const visibleEvents = events.filter(
  (event) => event.capacity != null && event.signedCount < event.capacity,
);
