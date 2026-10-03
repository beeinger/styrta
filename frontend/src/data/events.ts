import eventsData from "./events.json";

export type MeetupEvent = {
  id: string;
  emoji: string;
  title: string;
  signedCount: number;
  capacity: number;
  latitude: number;
  longitude: number;
  hostName: string;
  startsAt: string;
};

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
  (event) => event.signedCount < event.capacity,
);
